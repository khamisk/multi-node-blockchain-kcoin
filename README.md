# Multi-Node Blockchain - KCoin

A four-validator blockchain and live explorer built in Rust and React, with browser-signed transactions, 3-of-4 finality, independent persistent ledgers, and verified recovery after node downtime.

**Stack:** Rust, React, Axum, rust-libp2p/QUIC, SQLite, WebCrypto, Docker Compose

## TL;DR

KCoin is a working blockchain network that starts locally with one command.

- **Wallet:** The browser creates a private key and a matching public key. The private key signs transactions and never goes to a node. The public key is turned into the wallet address used to receive KCoin.
- **Earn:** Answer the current math challenge. A valid claim creates KCoin and adds it to the wallet balance.
- **Send:** Enter another wallet address and an amount. The wallet signs the transfer and sends it to the network.
- **Confirm:** Four independent validators check the transfer. When 3 of 4 accept the same block, it is final.
- **Explore:** The explorer shows wallets, transactions, blocks, balances, ownership, and validator activity.
- **Recover:** If one validator goes offline, the other three continue. When it returns, it downloads and verifies every block it missed before rejoining.

![Create a wallet, earn and send KCoin, inspect finality, then recover an offline validator](docs/assets/kcoin-demo-highlight.gif)

[Download the full 1:53 recording](https://github.com/khamisk/multi-node-blockchain-kcoin/releases/download/v0.1.0/kcoin-demo.webm). It uses four validator containers, one observer, and five separate SQLite databases. The wallet and chain data are real.

## What I built

- **Transactions:** Ed25519 signatures, Bech32m addresses, Borsh encoding, BLAKE3 hashes, nonce checks, expiry checks, state roots, and commit certificates.
- **Issuance:** Solving a deterministic arithmetic challenge creates KCoin. Rewards decrease across five supply bands until the 100,000 KCoin cap.
- **Consensus:** Four validators move through proposal, prevote, precommit, and finalize states. Three matching signatures finalize a block.
- **Networking:** libp2p gossip runs over QUIC. Returning nodes download certified blocks and verify them before voting again.
- **Storage:** Each node has its own SQLite database. Block finalization and signing records are written atomically.
- **Frontend:** The React app includes a browser wallet, explorer, validator recovery view, and ownership map. It uses an Axum REST/SSE API.

I implemented the protocol rules, consensus state machine, node runtime, storage and synchronization paths, API, wallet integration, explorer, tests, and Docker devnet. Established libraries provide the cryptographic primitives, canonical serialization, networking transport, and SQLite bindings.

## Run the network

Docker with Compose v2 is the only prerequisite.

```bash
docker compose up --build --detach --wait
```

Open [http://localhost:8080](http://localhost:8080). The observer API is available at `http://localhost:4100`.

This starts four validators, one non-voting observer/API node, the React app, and five independent SQLite volumes. To stop the network and delete its local chain data:

```bash
docker compose down --volumes
```

## How finality works

```mermaid
flowchart LR
    B["Browser wallet + explorer"]
    O["Observer / API<br/>Axum REST + SSE"]
    V["Four validators<br/>Proposal, prevote, precommit"]
    VD[("Independent validator<br/>SQLite databases")]
    OD[("Observer SQLite")]

    B -->|"signed transaction"| O
    O -->|"libp2p transaction gossip"| V
    V -->|"certified blocks"| O
    V --> VD
    O --> OD
    O -->|"finalized events + queries"| B
```

The browser generates an Ed25519 key and signs canonical transaction bytes locally; private key material never reaches a node. A rotating proposer places the transaction in a candidate block, and every validator independently checks its signature, nonce, balance, transaction root, and resulting state root.

Validators first prevote, then precommit. Three matching precommits from the fixed four-validator set form a commit certificate. That block is final immediately. There is no longest-chain fork choice or later reorganization.

[![Block detail showing the three-validator commit certificate and signature bytes](docs/assets/demo-04-explorer-certificate.png)](docs/assets/demo-04-explorer-certificate.png)

The safety model assumes no more than one Byzantine validator. One unavailable validator leaves a three-node quorum; a 2-2 partition halts instead of allowing either side to finalize alone.

## Validator failure and recovery

```bash
docker compose stop validator-4
```

Earn or send KCoin while it is offline. Validators 1, 2, and 3 still produce a three-signature certificate, so the explorer height advances. Then restart it:

```bash
docker compose start validator-4
```

The returning validator does not vote while behind. It requests finalized blocks and verifies every certificate, parent link, transaction, and state root before committing locally. Recovery is complete only when its height, finalized block hash, and state root match the network.

[![Validator 4 verifying 18 missed blocks while the other three remain current](docs/assets/demo-07-validator-syncing.png)](docs/assets/demo-07-validator-syncing.png)

See the [offline](docs/assets/demo-06-validator-offline.png) and [fully recovered](docs/assets/demo-08-validator-recovered.png) states.

## Tested evidence

| Property | Automated evidence |
| --- | --- |
| Forged, replayed, duplicate, expired, and overspent transactions are rejected without mutation | [Ledger tests](crates/kcoin-protocol/src/ledger.rs) |
| One equivocating validator cannot make honest validators finalize conflicting blocks | [Consensus simulations](crates/kcoin-consensus/src/simulation.rs) |
| Three validators continue finalizing while one is unavailable | [Simulations](crates/kcoin-consensus/src/simulation.rs) and [real libp2p integration](crates/kcoin-node/src/runtime.rs) |
| A late validator and observer verify missed history and converge on block hash and state root | [Runtime integration tests](crates/kcoin-node/src/runtime.rs) |
| Canonical history reconstructs ledger state and explorer projections | [Protocol replay](crates/kcoin-protocol/src/ledger.rs) and [storage tests](crates/kcoin-node/src/storage.rs) |
| Rust and frontend code agree on addresses, signing bytes, signatures, and transaction IDs | [Rust vectors](crates/kcoin-protocol/tests/golden_vectors.rs) and [frontend vectors](web/src/test/wallet-vectors.test.ts) |

The [threat and correctness matrix](docs/threat-model.md) lists the test coverage and remaining gaps.

```bash
cargo test --locked --workspace --all-targets
cd web
npm ci
npm test
npm run build
```

GitHub Actions also builds the complete Docker network, finalizes real transactions, proves progress with Validator 4 offline, restarts it, and compares the recovered height, block hash, and state root.

## Repository guide

| Path | Contents |
| --- | --- |
| [`crates/kcoin-protocol`](crates/kcoin-protocol/) | Transactions, cryptography, ledger execution, blocks, roots, and certificates |
| [`crates/kcoin-consensus`](crates/kcoin-consensus/) | Consensus state machine, quorum logic, locking, timers, and simulations |
| [`crates/kcoin-node`](crates/kcoin-node/) | Networking, persistence, synchronization, API, health, and metrics |
| [`crates/kcoin-cli`](crates/kcoin-cli/) | Verification, reindexing, wallet utilities, and latency smoke tooling |
| [`web`](web/) | Browser wallet, explorer, ownership view, and validator UI |

Start with the zero-prerequisite [blockchain guide (PDF)](docs/multi-node-blockchain-from-zero.pdf), then read the [architecture](docs/architecture.md), [protocol](docs/protocol.md), [persistence](docs/persistence.md), [synchronization](docs/synchronization.md), [benchmark methodology](docs/benchmarks.md), and [demo method](docs/demo.md).

## Scope

Not included: smart contracts, mining, staking, fees, dynamic validator membership, slashing, governance, public peer discovery, snapshot sync, production custody, and public-cloud hardening.

The included benchmark command is a sequential latency smoke harness, not a network-capacity benchmark. No throughput claim is published without reproducible saturation testing and committed raw results.

## License

Licensed under the [MIT License](LICENSE).
