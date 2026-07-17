# ADR 0001: Four-validator Tendermint-inspired finality

## Decision

Use a fixed set of four equal-power validators and proposal/prevote/precommit consensus with a three-signature commit certificate. Votes identify one canonical block hash. A block header carries an immutable declared proposer slot and construction round; signed proposal envelopes and the separate certificate authenticate the current carrier and finality round.

## Why

This topology makes Byzantine quorum reasoning, rotating production, deterministic finality, partitions, and recovery concrete. Four validators continue with one offline; a three-validator strict Byzantine quorum would require all three.

When a locked value moves to a later round, the proposer signs and carries the original block bytes instead of rewriting their header. This preserves one identity for votes, SQLite, explorer links, and parent hashes even if different nodes first observe valid certificates from different rounds. The explorer exposes declared header metadata separately from the certificate's finality round, and `verify_for_block` verifies that every certificate signs the immutable canonical ID. The header slot is deterministic metadata, not a retained signature proving which earlier envelope first introduced the bytes; that provenance system was excluded because it adds complexity without changing safety or observable recovery behavior.

Before any local proposal, prevote, or precommit is broadcast, the validator durably records its exact signing decision and safety state. The safety state includes both its lock and the distinct signed prevotes proving the latest valid round. Recovery authenticates that proof and carries it into a later proposal, which preserves liveness when honest validators held different locks before one restarted. Missing or forged proof fails startup closed.

The design adapts the locking, rotating proposer, and two-vote concepts from the [Tendermint paper](https://arxiv.org/pdf/1807.04938) and the [CometBFT consensus specification](https://docs.cometbft.com/v0.38/spec/consensus/consensus), while deliberately omitting dynamic validator membership, weighted voting, and production pacemaker machinery.

## Alternatives

- Raft is easier but only supports crash faults.
- HotStuff adds a more complex certificate tree and pacemaker without a corresponding safety or operational benefit at four nodes.
- Proof of work or stake adds economic machinery that distracts from the distributed-systems core.

## Exclusions

Dynamic membership, weighted voting, slashing, staking, threshold signatures, and public validator discovery.
