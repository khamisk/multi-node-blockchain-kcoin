# Persistence, verification, and reconstruction

Each KCoin process owns an independent SQLite database. The node's ledger actor is the only component that mutates ledger state or writes finalized history. This keeps deterministic execution separate from durable I/O.

## What is authoritative

The `blocks` table stores canonical Borsh block bytes and the separate commit-certificate bytes at every finalized height. This append-only history is the source of truth.

The following data is derived and rebuildable:

- account balances, nonces, display names, and transaction counts;
- transaction lookup rows;
- leaderboard order;
- current tip, state root, issued supply, and challenge metadata.

SQLite runs in WAL journal mode with `synchronous=FULL`, foreign keys enabled, and a busy timeout. These SQLite settings are distinct from KCoin's application-level consensus safety log.

## Atomic finalized commit

[`Store::persist_finalized`](../crates/kcoin-node/src/storage.rs) uses one SQLite transaction to:

1. reject a different block already stored at the same height;
2. insert canonical block and certificate bytes;
3. update transaction and account projections;
4. update tip, state-root, supply, and challenge metadata; and
5. clear the superseded in-progress consensus safety state.

If any write fails, none of the finalized state becomes visible. Repeating the identical block is idempotent; presenting a different canonical block at that height fails with `ConflictingFinality`.

The storage tests cover [atomic block/projection visibility](../crates/kcoin-node/src/storage.rs) and [conflicting finality](../crates/kcoin-node/src/storage.rs).

## Consensus write-ahead safety

SQLite's WAL protects database transactions. Separately, KCoin records **why a validator is allowed to sign** before it broadcasts anything.

For every local proposal, prevote, or precommit slot, `signer_state` stores:

- the exact canonical signing bytes;
- the resulting Ed25519 signature;
- the post-decision consensus `SafetyState` containing the round, lock, latest valid value, and its signed prevote quorum proof; and
- the complete signed consensus message to replay after restart.

Proposed block bytes and signed proposal envelopes are also retained by height and round. [`persist_consensus_decision`](../crates/kcoin-node/src/storage.rs) is idempotent for the same decision and fails closed if the same signer slot is presented with different signing bytes, safety state, or signed message. The runtime calls this persistence boundary in response to `PersistBeforeBroadcast` and only then feeds `Persisted` back to the consensus state machine ([driver](../crates/kcoin-node/src/runtime.rs)).

Tests cover [conflicting signing bytes](../crates/kcoin-node/src/storage.rs) and [signature/safety-state idempotence](../crates/kcoin-node/src/storage.rs).

## Restart restore behavior

Startup first verifies and replays finalized history. If active safety state remains for the next height, [`initialize_consensus_recovery`](../crates/kcoin-node/src/runtime.rs) then:

1. decodes the safety state and requires it to belong to exactly `finalized_height + 1`;
2. validates lock/valid-value consistency and authenticates every distinct signer in the persisted prevote quorum proof;
3. loads persisted proposals, verifies the current envelope signer/round, immutable declared header metadata and canonical ID, and revalidates block execution;
4. requires the bytes for every restored lock or valid value to be present;
5. loads each local signed decision, re-verifies the Ed25519 signature and exact signing bytes, and restores its signer slot;
6. requires the latest decision's safety bytes to match the singleton safety record;
7. starts the state machine from the restored round, lock, and valid-round proof, replays durable messages, and rebroadcasts the already-signed local messages.

Malformed, stale, incomplete, or internally inconsistent recovery data aborts startup instead of guessing. Identical signing requests reuse the stored signature; conflicting requests cannot overwrite it.

This behavior is covered by [`restart_restores_lock_and_reserves_prior_signer_slots`](../crates/kcoin-consensus/src/simulation.rs), the asymmetric-lock regression [`restart_preserves_newer_polc_and_breaks_asymmetric_lock_stall`](../crates/kcoin-consensus/src/simulation.rs), and the node-level SQLite recovery test [`restart_restores_lock_reuses_decisions_and_reproposes_in_next_round`](../crates/kcoin-node/src/runtime.rs). Storage invariants are tested, but an automated process-kill matrix at every signing/commit boundary is still a release evidence gap; see the [threat matrix](threat-model.md).

## Verify and reindex

[`verify_store`](../crates/kcoin-node/src/runtime.rs) is read-only. It decodes every stored block and certificate, verifies the fixed validator signatures and block transition from genesis, compares every duplicated block column against canonical bytes, and reports reconstructed height, canonical block hash, state root, supply, accounts, and transaction count.

[`reindex_store`](../crates/kcoin-node/src/runtime.rs) first validates the canonical block/certificate blobs without trusting duplicated explorer columns, then rebuilds transactions, accounts, metadata, and those duplicated columns from the canonical bytes. A final strict verification checks the repaired database. Canonical history is never rewritten.

With a stopped node database, the CLI surface is:

```bash
kcoin db verify --db /path/to/node.db --chain-id kcoin-local-1
kcoin db reindex --db /path/to/node.db --chain-id kcoin-local-1
```

The database must be stopped or copied consistently before offline inspection. Snapshotting, pruning, online migration, backup orchestration, and released-network schema compatibility are outside v1.
