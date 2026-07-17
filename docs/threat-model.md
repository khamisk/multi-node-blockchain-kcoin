# Threat model and correctness evidence

This document maps KCoin's safety properties to automated tests and notes the remaining gaps.

## Assumptions

- Fewer than one third of validator power is Byzantine. In the fixed four-validator set, safety tolerates one Byzantine validator and progress tolerates one unavailable validator.
- A quorum of correct validators can eventually communicate; an indefinite partition may halt progress.
- Wallet and validator private keys are not compromised.
- The local disk, operating system, Docker host, and genesis validator-key configuration are inside the operator trust boundary.
- Docker Compose uses four deterministic development validator keys. They are public and must not be reused outside the local devnet. Consensus tests assume uncompromised validator keys.
- Ed25519, BLAKE3, Borsh, Bech32m, SQLite, QUIC, and rust-libp2p are used through established libraries; KCoin does not implement custom cryptography.

## Evidence labels

- **Tested:** A committed automated test exercises the property.
- **Partial:** Enforcement and some tests exist, but the intended adversarial or process-level test coverage is not complete.
- **Not yet evidenced:** Do not describe the property as fully supported by automated evidence.

## Correctness matrix

| Scenario | Required result | Evidence | Status |
| --- | --- | --- | --- |
| Forged wallet signature | Reject before state mutation | [`replay_nonce_expiry_overspend_and_forgery_do_not_mutate_state`](../crates/kcoin-protocol/src/ledger.rs) | **Tested** |
| Forged validator vote | Reject before quorum/cache accounting | [`forged_vote_is_rejected_before_entering_cache`](../crates/kcoin-consensus/src/simulation.rs) | **Tested** |
| Replayed nonce, duplicate transaction, or cross-chain replay | At most one transaction finalizes | [Nonce/duplicate and cross-chain tests](../crates/kcoin-protocol/src/ledger.rs) | **Tested** |
| Expired transaction | Reject without mutation | [Ledger atomic-failure test](../crates/kcoin-protocol/src/ledger.rs) | **Tested** |
| Overspend | Reject without partial state change | [Ledger atomic-failure test](../crates/kcoin-protocol/src/ledger.rs) | **Tested** |
| Arithmetic overflow | Checked arithmetic returns a stable rejection instead of wrapping | Checked paths and the transfer-conservation property test are implemented in [`ledger.rs`](../crates/kcoin-protocol/src/ledger.rs), but there is no dedicated maximal-value overflow vector | **Partial** |
| Stale or duplicate reward claim | Only the first valid claim advances challenge and supply | [`first_valid_claim_wins_and_advances_the_persistent_challenge`](../crates/kcoin-protocol/src/ledger.rs) | **Tested** |
| Supply schedule or cap violation | Never exceed 100,000 KCoin | [`reward_bands_and_final_cap_are_exact`](../crates/kcoin-protocol/src/ledger.rs) | **Tested** |
| Invalid transaction root | Reject the block | [`tampered_block_is_rejected`](../crates/kcoin-protocol/src/block.rs) | **Tested** |
| Invalid post-state root | Reject the whole block without mutating ledger state | [`block_application_is_atomic_and_replay_reconstructs_the_same_state`](../crates/kcoin-protocol/src/ledger.rs) | **Tested** |
| Duplicate, unknown, out-of-order, or insufficient certificate signers | Never form quorum | [Certificate tests](../crates/kcoin-protocol/src/commit.rs), including duplicate, outsider, ordering, and two-of-four cases | **Tested** |
| A locked value is re-proposed in a later round | Preserve exact canonical bytes and parent identity while a later certificate records finality | [Protocol dual-certificate test](../crates/kcoin-protocol/src/commit.rs) and [node import-convergence test](../crates/kcoin-node/src/runtime.rs) | **Tested** |
| Duplicate or equivocating consensus vote | Count one vote and emit evidence for conflict | [`duplicate_and_equivocating_votes_count_only_once`](../crates/kcoin-consensus/src/simulation.rs) | **Tested** |
| One Byzantine validator sends conflicting proposals/votes | Honest validators do not finalize conflicting values | [`one_equivocator_cannot_make_honest_nodes_finalize_conflicts`](../crates/kcoin-consensus/src/simulation.rs) | **Tested** |
| One validator offline | Remaining three continue finalizing | [Virtual-time simulation](../crates/kcoin-consensus/src/simulation.rs) and [real libp2p integration](../crates/kcoin-node/src/runtime.rs) | **Tested** |
| 2-2 partition | Neither side finalizes; all converge after healing | [`two_two_partition_halts_then_converges_after_healing`](../crates/kcoin-consensus/src/simulation.rs) | **Tested** in simulator; no multi-process chaos test yet |
| Conflicting certified history | Halt instead of reorganizing or accepting later finalized-state mutation | [`conflicting_valid_certificate_halts_the_node`](../crates/kcoin-node/src/runtime.rs) sends a valid next block and watchdog event after the conflict; the [storage conflict test](../crates/kcoin-node/src/storage.rs) covers the durable boundary | **Tested** |
| Crash after local signing | Restore the lock and its authenticated valid-round proof, reuse an identical signature, and refuse a conflicting signer slot | [`restart_preserves_newer_polc_and_breaks_asymmetric_lock_stall`](../crates/kcoin-consensus/src/simulation.rs), [storage fail-closed test](../crates/kcoin-node/src/storage.rs), and [`restart_restores_lock_reuses_decisions_and_reproposes_in_next_round`](../crates/kcoin-node/src/runtime.rs) | **Partial.** Deterministic and SQLite restart paths are tested; no process-kill fault-injection matrix |
| Late validator or observer | Verify missed certified blocks and converge on hash/state root before voting | [`three_of_four_real_network_finalizes_and_observer_catches_up`](../crates/kcoin-node/src/runtime.rs) | **Tested** for valid catch-up |
| Wrong-kind, stale, or misattributed sync reply | Reject it and retry only if it still matches the active request | Real-QUIC [`response_event_preserves_the_exact_outbound_request_intent`](../crates/kcoin-node/src/network.rs), plus [`blocks_request_answered_with_status_is_rejected_for_peer_rotation`](../crates/kcoin-node/src/runtime.rs) and stale ID/peer/range regressions | **Tested** |
| Dropped sync completion or repeated gap hints | Preserve one healthy range request; use an ID-bound deadline to recover lost completion without starving or cancelling its successor | Real-QUIC [`active_sync_request_survives_gap_hints_and_watchdog_recovers_lost_completion`](../crates/kcoin-node/src/runtime.rs) | **Tested** |
| Corrupt sync range | Reject without committing bad history and retry through the bounded peer path | Shared certificate/replay verification in [`import_finalized`](../crates/kcoin-node/src/runtime.rs) and strict sequence validation in [`stale_valid_sync_range_is_rejected_before_retrying_another_peer`](../crates/kcoin-node/src/runtime.rs) | **Partial.** No adversarial network fixture injects a malformed certified record yet |
| Forged, cross-chain, or uncommitted future-height hint | Never enter non-voting sync mode without a verified finalized certificate for the configured chain | [`untrusted_consensus_hints_cannot_poison_slots_or_force_syncing`](../crates/kcoin-node/src/runtime.rs) covers wrong proposers, out-of-window rounds, future proposals, unsigned status claims, fully certified wrong-chain tips, and cross-chain signed validator status | **Tested** |
| Full replay and projection rebuild | Reproduce finalized height, canonical block hash, state root, balances, indexes, and duplicated block columns | [Pure replay test](../crates/kcoin-protocol/src/ledger.rs), [`verify_store` / `reindex_store` projection-corruption regression](../crates/kcoin-node/src/runtime.rs) | **Tested** in-process; the persisted CLI path is a manual release smoke, not a CI claim |
| Rust/browser signing compatibility | Match address, signing bytes, signature, and transaction ID | [Rust golden test](../crates/kcoin-protocol/tests/golden_vectors.rs) and [Vitest vectors](../web/src/test/wallet-vectors.test.ts) | **Tested** |
| Malformed or oversized decoders | Reject boundedly | [Transaction decoder test](../crates/kcoin-protocol/src/transaction.rs), randomized [transaction/block/certificate decoder properties](../crates/kcoin-protocol/tests/decoder_properties.rs), size checks in protocol decoders, and [network envelope round-trip](../crates/kcoin-node/src/network.rs) | **Partial.** Persistent coverage-guided fuzz targets are not committed |

## Why 3-of-4 finality is safe

Three of four precommits are required. Any two quorums of three overlap in at least two validators; with at most one Byzantine validator, the overlap includes an honest validator. The lock and valid-round rules prevent that honest validator from precommitting conflicting values without a newer proof that itself required a quorum. The implementation is a deterministic event/effect state machine so persistence and networking cannot silently change the signing decision.

The certificate signs the same canonical block hash used by storage and parent links. A later proposer must carry a locked block byte-for-byte, so valid certificates from two rounds cannot split canonical history. The signed proposal envelope authenticates the current rotating proposer; the immutable block header retains declared construction metadata, and the certificate records the round that finalized it. The header fields are not claimed as independently authenticated first-proposal provenance. See [Protocol: blocks and certificates](protocol.md#blocks).

## Out of scope

Production traffic economics, Sybil-resistant public membership, validator slashing, validator-key rotation, secret-key recovery, smart-contract isolation, public-cloud hardening, internet-scale denial-of-service resistance, snapshot trust, and protection from a compromised host are outside v1.

Until the **Partial** rows gain their named automated tests, public documentation should describe the implemented mechanisms without claiming the entire correctness matrix is complete.
