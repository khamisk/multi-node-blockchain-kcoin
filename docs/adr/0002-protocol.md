# ADR 0002: Canonical Rust protocol core

## Decision

Keep all consensus-critical types and execution in a side-effect-free Rust crate. Use Borsh, BLAKE3, Ed25519, and integer atoms. The browser keeps custody in WebCrypto and uses a narrow TypeScript encoder checked byte-for-byte against committed Rust golden vectors for addresses, signing bytes, Ed25519 signatures, and transaction IDs.

## Why

Rust remains authoritative. Cross-language vectors make drift fail in both Rust and browser CI while keeping the frontend's one-command build understandable and avoiding a generated WASM toolchain in the primary demo path. Optional WASM exports remain available in the protocol crate if the browser surface later grows beyond this small adapter.

## Alternatives

- Runtime WASM reuse removes the handwritten adapter but adds generated glue and another build tool to the primary quickstart. For three bounded transaction variants, reciprocal golden tests provide a clearer tradeoff.
- Signing REST JSON was rejected because key order, numeric representation, and JavaScript number limits would make the consensus boundary ambiguous.

## Exclusions

Custom cryptographic primitives, JSON signing, floating-point balances, smart contracts, fees, arbitrary token types, and claims that the current browser bundle executes Rust/WASM.
