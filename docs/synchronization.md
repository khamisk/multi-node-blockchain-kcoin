# Synchronization and downtime recovery

KCoin synchronizes finalized history, not competing forks. A node either verifies the unique certificate-backed chain or halts on conflicting finality.

## Transport

[`kcoin-node::network`](../crates/kcoin-node/src/network.rs) runs rust-libp2p over QUIC with:

- strict, signed Gossipsub for transactions, consensus messages, finalized blocks, and validator status;
- Identify and Ping;
- chain-scoped, versioned Borsh gossip envelopes; and
- a versioned CBOR request-response protocol at `/kcoin/sync/1`.

The local Docker network uses static bootstrap multiaddresses. Each node retries disconnected bootstrap addresses every three seconds, with at most four new dials per pass. A completed outbound dial associates that address with the authenticated libp2p peer ID; while any connection to that peer remains active, later passes skip the address. If the process restarts with a new node identity, the next address-based dial learns the replacement identity. This keeps earlier validators reachable after restart without churning healthy QUIC connections.

libp2p peer identity authenticates the transport connection; validator Ed25519 keys separately authorize consensus votes and validator-status messages. The allowlist mechanism exists in the network layer, but the current local CLI starts it empty, so this repository does not claim production peer admission control.

## Sync protocol

The request-response protocol has two bounded operations:

- `Status` returns a peer's claimed finalized height, canonical block hash, state root, whether it is itself syncing, and the canonical block/certificate record that proves a non-genesis tip. A higher status cannot establish `sync_target` unless that attached certificate and all redundant fields verify.
- `Blocks { from_height, limit }` returns canonical block/certificate records in ascending order. The storage layer caps a range at 128 blocks.

Every outbound request receives a stable local ID and retains its expected peer, operation, starting height, and limit alongside libp2p's request ID. The ledger actor accepts a block response only when all of that intent still matches the active catch-up request. A `Status` reply to a `Blocks` request, a delayed reply from an older request, a reply attributed to another peer, or a stale/oversized range is rejected before decoding or mutation.

Only one block-range request is active at a time. New certified gap hints raise the target height without replacing healthy in-flight work, and newly connected peers do the same. A 12-second actor-side deadline recovers if the network event bus drops a response/failure completion under load; the retry is bound to the exact active ID, so an old deadline cannot cancel its successor. An active failure rotates to another connected peer; if only the same peer remains, the node retries it after a bounded 250 ms delay. A disconnected peer is never retried until it reconnects.

A node begins sync only after it verifies a certificate-backed finalized record for its configured chain, either attached to a status response or received through finalized gossip. Future-height proposals, votes, signed single-validator telemetry, cross-chain certificates, and status responses without a matching tip certificate cannot establish `sync_target`. Signed validator-status telemetry also includes the chain ID, preventing a valid status from one local network being rewrapped for another. While a validator has a certificate-backed sync target, the runtime refuses to start or process consensus and reports phase `syncing`.

## Verification before commit

Each received finalized record is decoded and checked through the same protocol path used for local finality:

1. block size, version, chain, transaction root, and canonical encoding;
2. three unique authorized certificate signatures over the canonical block hash;
3. certificate height/finality round plus valid immutable declared header metadata;
4. the next sequential height and canonical parent hash;
5. every wallet signature, nonce, expiry, challenge, balance, and checked state transition; and
6. the post-execution state root.

Only then does the node atomically commit the canonical bytes and projections. It asks for another bounded range until local height reaches the target, then clears syncing mode and allows a validator to enter consensus for `height + 1`.

If a certified block conflicts with an already finalized canonical block at the same height, the node enters a fail-closed halt. It clears synchronization state and refuses later consensus, transaction admission, synchronization, and finalized-state mutation; it does not reorganize or keep appending. Invalid peer data is rejected and the active request is retried through the bounded peer-selection path. Request/response intent, target preservation, lost-completion recovery, and successor-deadline isolation are covered over real QUIC, with additional wrong-kind, stale-ID, wrong-peer, and stale-range regressions. Persistent peer scoring and a network fixture that injects a malformed but correctly framed certified range remain outside the demonstrated evidence.

## Recovery demonstration

The real-network integration test [`three_of_four_real_network_finalizes_and_observer_catches_up`](../crates/kcoin-node/src/runtime.rs) starts three independent validator instances over real libp2p/QUIC transports inside one Tokio test process, finalizes while the fourth validator is absent, then starts the missing validator and a fresh observer. Both download certified history and end with the same canonical block hash and state root. Multi-process Docker recovery is the manual demo path, not what this automated test claims.

The Docker demonstration exposes the same operator flow:

```bash
docker compose stop validator-4
# Submit work or wait for a heartbeat block; Validators 1-3 retain quorum.
docker compose start validator-4
```

The React validator rail reads signed status observations from the observer and shows offline, syncing, and current states. Recovery is complete only when height, canonical finalized hash, and state root match.

## Not included

There is no public peer discovery, NAT hardening, snapshot sync, pruning, peer reputation, or dynamic validator membership. V1 targets a fixed local network, not a public deployment.
