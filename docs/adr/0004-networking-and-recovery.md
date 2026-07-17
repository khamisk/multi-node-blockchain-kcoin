# ADR 0004: libp2p gossip plus verified range synchronization

## Decision

KCoin uses rust-libp2p over QUIC. Strict signed Gossipsub carries bounded transactions, consensus messages, finalized blocks, and validator status. A versioned request-response protocol exchanges status and bounded finalized-block ranges. Validator keys authorize votes; libp2p peer identities authenticate transport identity separately.

Configured static bootstrap addresses are connection-maintenance targets, not one-shot startup hints. The network task checks them every three seconds and starts at most four dials per pass. It records the authenticated peer ID from each successful outbound connection and suppresses further address dials while that peer remains connected. A disconnected address is dialed without pinning the previous node identity, allowing a restarted local peer to reconnect even when it generated a new libp2p key.

A lagging validator or observer stays non-voting while it requests missing ranges. It verifies every certificate, parent link, transaction, and state root before committing the range and joining the current height.

The node also retains each outbound sync request's local ID, expected peer, request kind, starting height, limit, and deadline. Responses and transport failures carry that intent back to the ledger actor, which ignores stale traffic and rejects mismatched response kinds or ranges before mutation. Repeated gap hints raise the target without replacing active work. A watchdog recovers a completion event lost under queue pressure, while ID-bound deadlines prevent stale timers from disturbing successor requests. Active failures rotate peers, with a bounded retry when only one connected provider exists.

## Alternatives considered

- A custom TCP protocol would move framing, connection management, and transport concerns into application code.
- Longest-chain synchronization conflicts with immediate BFT finality and introduces unnecessary fork choice.
- Database copying is fast locally but cannot prove that a recovering node validated history.

## Engineering consequences

The approach exposes peer propagation, bounded protocols, independent identities, catch-up, and recovery after downtime as observable system behavior.

## Deliberate exclusions

Public discovery, dynamic membership, NAT traversal hardening, persistent peer reputation, snapshot sync, and adversarial internet deployment are outside v1. A failed or corrupt sync peer is excluded from the immediate retry choice, but KCoin does not implement a production-grade provider-scoring scheduler. The network layer accepts an allowlist, but the local CLI currently leaves it empty; production peer admission is not claimed.
