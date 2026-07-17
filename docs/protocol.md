# Protocol v1

This document describes consensus-critical behavior implemented by [`kcoin-protocol`](../crates/kcoin-protocol/src/lib.rs). REST JSON is an adapter and is never signed directly.

## Encoding and hashing

- Consensus objects use canonical [Borsh](https://borsh.io/) encoding.
- Hashes are 32-byte BLAKE3 outputs with distinct versioned domain strings for addresses, transaction IDs, Merkle nodes, state roots, and canonical block IDs.
- Wallet transactions and commit votes include a protocol version and chain ID. Consensus proposal and prevote envelopes are domain-separated by canonical enum variant and also include the chain ID.
- Maps, floating-point numbers, platform-sized integers, and unordered collections do not appear in signed structures.
- REST renders 64-bit protocol quantities as decimal strings so JavaScript never rounds them.

The authoritative implementations live in [`transaction.rs`](../crates/kcoin-protocol/src/transaction.rs), [`crypto.rs`](../crates/kcoin-protocol/src/crypto.rs), and [`block.rs`](../crates/kcoin-protocol/src/block.rs). Rust and browser compatibility is pinned by [checked-in golden vectors](../crates/kcoin-protocol/test-vectors/wallet.json), a [Rust test](../crates/kcoin-protocol/tests/golden_vectors.rs), and a [browser test](../web/src/test/wallet-vectors.test.ts).

## Wallets and addresses

An address is Bech32m with human-readable prefix `kcoin`. Its payload is the first 20 bytes of a domain-separated BLAKE3 hash of the wallet's 32-byte Ed25519 public key. A signed transaction includes the full public key; validators derive the sender address rather than trusting a redundant address field.

The browser generates Ed25519 keys with WebCrypto, requires a downloadable PKCS#8 JSON backup before enabling signing, and otherwise retains the active key only in memory. The TypeScript adapter implements the same bounded canonical encodings as Rust and is checked against Rust vectors. Optional Rust/WASM exports exist, but the current browser build does **not** claim to execute the protocol crate through WASM; [ADR 0002](adr/0002-protocol.md) records that tradeoff.

Wallets may have optional public display names shown on the explorer, leaderboard, and ownership map. A signed `SetDisplayName` transaction publishes or clears one. Display names are cosmetic, need not be unique, and are never valid destinations; transactions must still use `kcoin1...` wallet addresses.

## Transactions

`UnsignedTransaction` contains:

- protocol version;
- chain ID;
- sender Ed25519 public key;
- the sender's next account nonce;
- expiry height; and
- exactly one action.

The actions are:

- `Transfer { recipient, amount_atoms }` — moves a positive integer number of atoms;
- `ClaimReward { challenge_id, answer }` — claims the single active arithmetic challenge; or
- `SetDisplayName { display_name }` — updates cosmetic public metadata without moving value.

`SignedTransaction` adds a 64-byte Ed25519 signature. The signature covers a versioned prefix plus canonical unsigned bytes; the transaction ID hashes the canonical signed representation. Validation checks shape, chain, expiry, signature, duplicate ID, exact nonce, action semantics, balance, and checked arithmetic before mutating state.

Transactions execute sequentially in block order. One invalid transaction invalidates the proposed block. Proposers therefore simulate candidate execution and omit invalid mempool entries.

Stable API rejection codes are derived from [`ValidationError::code`](../crates/kcoin-protocol/src/error.rs), including `INVALID_SIGNATURE`, `NONCE_MISMATCH`, `INSUFFICIENT_BALANCE`, `STALE_CHALLENGE`, `EXPIRED`, and `MALFORMED`.

## REST boundary

The Axum adapter exposes status, validators, the active challenge, blocks, transactions, addresses, a balance leaderboard, and finalized SSE events under `/api/v1`, plus liveness, readiness, and Prometheus metrics endpoints. Block history uses a descending height cursor, transaction history uses a stable `height:index` cursor, and the leaderboard uses a decimal row-offset cursor. Each page fetches one extra record so `next_cursor` is returned only when older data exists.

All 64-bit values and platform-sized counts are rendered as decimal strings in JSON; protocol versions, validator indexes, consensus rounds, ranks, and basis-point percentages remain bounded JSON numbers. Malformed request JSON, invalid decimal strings, and malformed cursors return the same `{ "code", "message" }` error shape used by protocol validation. Transaction submission returns HTTP 503 with `NODE_SYNCING` or `NODE_HALTED` while the observer cannot safely admit writes; the browser reflects the same state and disables signing. REST DTOs are presentation-only: signatures always cover Rust-generated canonical Borsh bytes, never JSON.

## Supply and challenges

One KCoin is `1,000,000` atoms. Supply starts at zero, there are no fees or premine, and the maximum is exactly `100,000 KCoin`.

| Supply before a claim | Reward |
| --- | ---: |
| `0 <= supply < 20,000` KCoin | 100 KCoin |
| `20,000 <= supply < 40,000` KCoin | 50 KCoin |
| `40,000 <= supply < 60,000` KCoin | 25 KCoin |
| `60,000 <= supply < 80,000` KCoin | 10 KCoin |
| `80,000 <= supply < 100,000` KCoin | 5 KCoin |

The final reward is reduced if necessary so issuance lands exactly on the hard cap.

There is one active, deterministic one-digit addition, subtraction, or multiplication challenge. It persists until the first valid claim executes in a finalized block. Challenge content is a pure function of its monotonically increasing ID; after a winning claim, the ID increments and every node derives the same next challenge without randomness or clocks. Proposer ordering and anti-bot fairness are deliberate v1 exclusions.

## Blocks

A block contains an ordered transaction list and a header with protocol version, chain ID, height, parent hash, declared proposer, construction round, timestamp, transaction Merkle root, and post-execution state root. Those proposer/round fields are immutable deterministic header metadata. A later proposal envelope authenticates the validator carrying the bytes in its current round, while the commit certificate records the round that finalized them. KCoin does not retain a separate proof that the declared header slot was the first envelope to introduce the bytes.

The timestamp is proposed data, not a source of local nondeterminism: validators check the same bytes and derive the same result. After the first block it must be nondecreasing and advance by at most 60 seconds; honest construction clamps wall time into that deterministic range, so a future-dated parent cannot halt block production. V1 also caps timestamps at 3000-01-01. The transaction root commits to order and content; the state root commits to the full deterministic ledger state after execution.

KCoin exposes one canonical block commitment:

- `Block::hash()` covers the complete header, including its declared proposer and construction round, and is used for proposal IDs, votes, locks, persistence, explorer links, and the next block's parent hash.
- `Block::consensus_hash()` is an explicit alias for that same identity at consensus call sites.

When a later proposer carries a locked value into a new round, it signs a new proposal envelope around the original block ID and exact original block bytes. The later round and proposer therefore do not rewrite canonical history. [`certificates_from_two_rounds_authenticate_one_canonical_block`](../crates/kcoin-protocol/src/commit.rs) proves two independently valid round certificates converge on one block and parent identity.

## Commit certificates

The validator set is fixed and equal-power in v1. Quorum is `floor(2N/3) + 1`; four validators therefore require three unique authorized signatures.

A `CommitCertificate` stores protocol version, chain ID, height, finality round, canonical `block_hash`, and sorted validator precommit signatures. Its round may equal or follow the block's construction round because a quorum can finalize unchanged locked bytes after a timeout.

Certificate validation rejects the wrong chain, height, block hash, an impossible round earlier than the block's construction round, invalid declared rotation metadata, malformed or duplicate signers, unknown validator keys, invalid Ed25519 signatures, non-canonical signer order, and fewer than three signatures. The consensus state machine separately verifies that each live proposal envelope was signed by the expected proposer for its current round. Certificates remain separate from block bytes, avoiding a circular block-hash dependency and allowing more than one valid proof for the same immutable block.

The REST explorer labels `proposer` and `round` as finality metadata derived from the certificate and fixed rotation. It returns `header_proposer` and `header_round` for the immutable declared header slot, explicitly avoiding an unsupported provenance claim.

## Deterministic replay

Starting from genesis, a node decodes each canonical block and certificate, verifies the certificate against the fixed validator set, checks the parent link and commitments, executes every transaction in order, and compares the computed state root. The final block hash, state root, balances, nonces, supply, challenge, display names, and transaction set are outputs of that replay. [Persistence](persistence.md) explains how `verify` and `reindex` use this property.
