# ADR 0003: Canonical blocks with rebuildable SQLite projections

## Decision

Each node owns one SQLite database. A single ledger actor serializes validation and writes. Finalized block bytes, their commit certificate, and tip metadata are committed atomically with query projections while SQLite runs in WAL mode with full synchronization. `verify` replays canonical history without changing the database; `reindex` verifies first and then replaces derived account and transaction projections from that replay.

Consensus signing safety is a separate application-level write-ahead boundary. Before broadcast, the node stores the exact signing bytes, signature, signed message, and resulting lock/round state. Restart validates and restores that state, reserves previously used signer slots, replays durable messages, and rebroadcasts the saved signature. It fails closed if a lock, proposal, signature, or safety record is missing or inconsistent.

## Data flow

1. The ledger executes a candidate block in memory and checks its post-state root.
2. A quorum certificate makes that block final.
3. One SQLite transaction stores the canonical bytes, certificate, new tip, and projections.
4. The same transaction clears in-progress safety state superseded by the finalized height.
5. On restart, sync, or audit, the node decodes and replays canonical blocks from genesis.

## Alternatives considered

- A key-value database reduced schema work but made explorer projections and deterministic reconstruction less explicit.
- Treating projection tables as authoritative made reads simple but weakened the deterministic-reconstruction story.
- Multiple writers added concurrency without improving correctness or query capability and made partial-state bugs harder to exclude.

## Engineering consequences

This boundary provides persistent block storage, atomic finalization, independent node databases, and deterministic ledger reconstruction.

## Deliberate exclusions

Snapshots, pruning, migrations across released public networks, remote replicas, and production backup orchestration are outside v1. Unit tests cover fail-closed signer slots and lock restoration; a process-kill fault-injection matrix remains release work.
