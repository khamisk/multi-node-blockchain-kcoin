# Benchmark methodology

This repository includes a sequential latency smoke test. It is not a saturation benchmark, so no throughput result is published.

## Current harness

`kcoin benchmark` in [`kcoin-cli`](../crates/kcoin-cli/src/main.rs) can:

- query the running node's chain, height, peer count, and reported online validators;
- create or load a wallet and claim a challenge if funding is needed;
- submit sequential one-atom transfers at concurrency one;
- poll the transaction endpoint until each reports finalized;
- report observed p50, p95, and p99 client wall-clock latency plus sequential finalizations per second; and
- save raw per-sample latency JSON with timestamp, platform, commit SHA when available, node URL, node topology observations, and workload fields.

Example against the Docker observer API:

```bash
cargo run --release -p kcoin-cli -- benchmark \
  --api-url http://127.0.0.1:4100 \
  --samples 100 \
  --output benchmark-results/local/smoke.json
```

That output is a **concurrency-one observation**. It does not discover saturation, does not measure concurrent offered load, and does not collect CPU, memory, network, or database-growth telemetry. It must not be rewritten as network capacity TPS.

## Confirmation definition

For the current CLI, a sample starts immediately before HTTP submission and stops when `GET /api/v1/transactions/{id}` reports finalized on the measured API node. Against the default Docker topology that API belongs to the observer, whose transaction row is created only after it has received, verified, and committed the certified block.

For a publishable benchmark, confirmation must still be cross-checked against observer durability: the observer has verified a three-signature certificate and its SQLite commit has completed. Mempool acceptance, proposal receipt, prevotes, and a validator-only response are not confirmations.

Any future published result must include its raw samples, exact commit, machine and Docker environment, workload settings, repeated runs, and every offered-load point through saturation. Throughput must mean transactions durably finalized by the observer, not requests merely accepted by HTTP.
