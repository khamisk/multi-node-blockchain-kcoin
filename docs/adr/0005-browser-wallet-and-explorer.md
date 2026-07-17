# ADR 0005: Memory-only browser wallet inside the explorer

## Decision

The React application opens on the working network, not a marketing page. It combines historical REST reads with SSE invalidation and keeps the four-validator convergence rail visible across routes. Wallet keys are created with browser WebCrypto, exported once as an explicit PKCS#8 backup, and otherwise remain only in the active tab. Signed public display names are cosmetic; every transfer still targets a Bech32m address.

The visual system uses off-white surfaces, ink typography, thin rules, square controls, and one amber issuance accent. Ownership is shown both as area-proportional packed circles and an accessible table.

## Alternatives considered

- Node-managed keys would simplify the demo but invalidate the wallet-security story.
- Persistent browser storage would be convenient but imply a custody product v1 does not provide.
- A separate landing page would place introductory content before the working network.
- Decorative dashboards made the validator state less legible than a restrained engineering instrument.

## Engineering consequences

This makes the end-to-end behavior visible: locally generated signatures cause real ledger changes, explorer activity, supply metrics, ownership changes, and validator recovery feedback.

## Deliberate exclusions

Seed phrases, extensions, hardware wallets, cloud recovery, production custody, exchange data, and mobile apps are outside v1.
