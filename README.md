# Portaldot-IntentFlow
Users submit only their transaction intent, while a Rust Solver generates routing based on multi-pool liquidity, price impact, and execution risk. A Portaldot ink! contract then verifies the route constraints and executes secure settlement.

## Current scope

The first phase targets a single Portaldot network. Cross-chain routing is out of scope until the single-chain testnet flow is verified. The independent Rust workspace contains chain-neutral domain types, constant-product quotes, one/two-hop route search, deterministic input optimization and risk scoring, Portaldot adapter boundaries, guarded execution interfaces, and evidence records.

The Portaldot testnet's ink! capabilities have not yet been verified. Contract implementation and deployment are gated on the P0 probe; the existing contract directories are plans, not deployed code. Transaction execution is disabled in the supplied configuration.

## Workspace layout

- `crates/defi-domain`: network, asset, pool snapshot, intent, route, and evidence models.
- `crates/defi-engine`: quote, route search, optimization, risk scoring, and deterministic fingerprints.
- `crates/portaldot-client` and `crates/portaldot-dex`: chain and pool adapter interfaces.
- `crates/portaldot-solver`: risk-aware route selection.
- `crates/portaldot-executor`: dry-run-gated IntentRouter call boundary.
- `crates/evidence-store`: evidence schema and in-memory adapter.
- `contracts/`: P0-gated ink! contract plan.
- `docs/phase-1-development.zh-CN.md`: first-phase engineering plan.
- `docs/migration-map.zh-CN.md`: old platform migration decisions.

## Build

With Rust stable and crates.io dependencies available, run `cargo check --workspace` from the repository root. This workspace currently contains no concrete Portaldot RPC client and no deployable ink! contract; those follow the P0 runtime capability probe.
