# Three-minute demo and media capture

## Media status

The README includes an [18-second GIF](assets/kcoin-demo-highlight.gif) built from eight screenshots. The [full 1:53 recording](https://github.com/khamisk/multi-node-blockchain-kcoin/releases/download/v0.1.0/kcoin-demo.webm) is available as a release download. Both were captured from the Docker network with demo mode off.

Rebuild the GIF with `scripts/build-demo-highlight.ps1`. It requires Python and Pillow. The GIF uses still frames; the WebM is the continuous recording.

## Opening capture checklist

| Time | Action | Required visible evidence |
| ---: | --- | --- |
| 0-4s | Open Overview | Four validators show current with matching height, finalized hash, and state root |
| 4-8s | Create sender wallet and save backup | New Bech32m address; backup requirement changes to acknowledged |
| 8-14s | Solve active challenge | Claim appears pending, then finalized; balance and circulating supply increase |
| 14-18s | Open Ownership | Leaderboard/table and circle areas reflect the new balance |
| 18-23s | Send to a prepared recipient address | Transfer changes pending to finalized; explorer transaction opens |
| 23-28s | Run `docker compose stop validator-4` | Validator 4 becomes offline; another block finalizes with three validators |
| 28-38s | Run `docker compose start validator-4` | Rail shows offline -> syncing -> current and ends with matching height/hash/root |

Capture prerequisites:

- Start from fresh volumes so the issuance change is visually obvious.
- Prepare the recipient wallet in a second browser profile and copy its address before recording.
- Keep a terminal beside or below the browser with commands large enough to read.
- Confirm the web app is using `http://localhost:4100`, not its isolated frontend demo transport.
- Trigger a transfer while Validator 4 is stopped and show the new finalized block.
- End on all four validators aligned; do not cut away during syncing.

## Guided three-minute walkthrough

### 0:00-0:30: Prove the product exists

1. Run `docker compose up --build -d` and open `http://localhost:8080`.
2. Point out the four-validator rail: online state, phase, finalized height, block hash, state root, and lag.
3. Create the sender wallet, download the PKCS#8 JSON backup, and explain in one sentence that the private key stays in browser memory and transactions still target addresses.

### 0:30-1:15: Earn and transfer

1. Publish an optional display name, noting that it is cosmetic and non-unique.
2. Solve the active arithmetic challenge and submit the claim.
3. Follow pending to finalized; open its transaction and block certificate.
4. Show the wallet balance, circulating supply, leaderboard, and ownership table/map update.
5. Send KCoin to the recipient wallet prepared in another browser profile and open the finalized transfer.

### 1:15-2:05: Demonstrate quorum and recovery

Stop one validator:

```bash
docker compose stop validator-4
```

Submit another transfer or wait for a heartbeat block, then show Validators 1-3 advance. Explain: three of four precommits are a quorum; there is no longest-chain confirmation wait.

Restart it:

```bash
docker compose start validator-4
```

Show Validator 4 remain non-voting while it requests bounded finalized ranges and verifies certificates, parent links, transactions, and roots. Finish only when it is current and the rail's height, canonical finalized hash, and state root match.

### 2:05-3:00: Show the implementation

1. Open [`crates/kcoin-consensus/src/engine.rs`](../crates/kcoin-consensus/src/engine.rs) and point to the event/effect boundary: consensus requests persistence before broadcast.
2. Open [`crates/kcoin-node/src/storage.rs`](../crates/kcoin-node/src/storage.rs) and show exact signing bytes, signature, signed message, and safety state committed together.
3. Run `cargo test --workspace` and point to the [evidence matrix](threat-model.md), including its clearly marked remaining gaps.
4. Explain `kcoin db verify` versus `kcoin db reindex` using [persistence.md](persistence.md).
5. Show [benchmark methodology](benchmarks.md). Do not quote a TPS or p99 number until raw release results are committed.

## Capture acceptance

Before adding media to the README:

- verify the recording came from the real observer and four independent node databases;
- ensure no private-key backup content, local username, unrelated terminal history, or secret is visible;
- confirm every transaction shown in the clip can be found in the explorer;
- verify Validator 4's final height, canonical block hash, and state root match the other validators;
- add alt text or an adjacent text transcript; and
- keep this checklist linked so the demonstration remains reproducible.
