# Portaldot-IntentFlow
Users submit only their transaction intent, while a Rust Solver generates routing based on multi-pool liquidity, price impact, and execution risk. A Portaldot ink! contract then verifies the route constraints and executes secure settlement.

## Current scope

The first phase targets a single Portaldot network. Cross-chain routing is out of scope until the single-chain testnet flow is verified. The Rust workspace contains chain-neutral domain types, constant-product quotes, one/two-hop route search, deterministic input optimization and risk scoring, typed Portaldot configuration, a read-only Subxt client, dry-run-gated execution, and SQLite evidence persistence.

The P0 probe verified the single-contract upload, instantiation, calls, and event lifecycle. A second independent instance remains inconclusive because the account balance was below its storage deposit. P1 is in place. P2 now contains a test-only PSP22 token, a constant-product pool, a constrained one/two-hop Router, a shared ABI/hash crate, and local unit tests. P3 build artifacts and deployment tooling are present. Live deployment remains pending testnet funding; the latest read-only preflight is recorded in `evidence/p3/p2-deployment.json`. The deployment script defaults to preflight and requires explicit broadcast flags plus environment-injected credentials.

Structured tracing is emitted at adapter, solver, and executor boundaries. It omits RPC endpoint values, account identifiers, trade amounts, call data, and signed payloads.

## Workspace layout

- `crates/defi-domain`: network, asset, pool snapshot, intent, route, and evidence models.
- `crates/defi-engine`: quote, route search, optimization, risk scoring, and deterministic fingerprints.
- `crates/portaldot-client` and `crates/portaldot-dex`: chain and pool adapter interfaces.
- `crates/portaldot-config`: typed TOML configuration and environment-backed endpoint resolution.
- `crates/portaldot-solver`: risk-aware route selection.
- `crates/portaldot-executor`: metadata-backed IntentRouter encoding, dry-run checks, and lifecycle evidence orchestration.
- `crates/evidence-store`: in-memory and SQLite evidence stores.
- `apps/dashboard`: local read-only operations view over persisted evidence.
- `contracts/`: P2 ink! source and local test workspace; P3 deployment is still pending funding and a broadcast run.
- `docs/phase-1-development.zh-CN.md`: first-phase engineering plan.
- `docs/migration-map.zh-CN.md`: old platform migration decisions.
- `docs/p6-p8-operations.zh-CN.md`: P6–P8 local operations and acceptance boundaries.

## Build

With Rust stable and crates.io dependencies available, run `cargo check --workspace` and `cargo run -p portaldot-config --example validate -- config/portaldot-testnet.toml` from the repository root. P2 tests run with `cargo test --manifest-path contracts/Cargo.toml --workspace --locked`; build deployable artifacts with `scripts/build-p2-contracts.ps1`. On Windows the build uses the pinned Linux toolchain in Docker; elsewhere it requires `cargo-contract 4.1.3` and the `wasm32-unknown-unknown` target. Install deployment dependencies with `npm ci`.

Set `PORTALDOT_TESTNET_WS_URL` and `PORTALDOT_TEST_ACCOUNT_ADDRESS` in the environment, then run `npm run p2:deploy`. This performs read-only runtime checks and constructor/call dry-runs and writes `evidence/p3/p2-deployment.json`; it does not read the mnemonic, sign, or broadcast. Assuming all three code hashes need first-time storage, the final 63,945-byte Wasm set costs about 3,836.70 tPOT in code deposits, excluding contract state deposits and transaction fees. The latest preflight observed about 93.57 tPOT, leaving a code-deposit-only shortfall of about 3,743.13 tPOT. After funding, rerun the preflight with `npm run p2:deploy -- --resume` to get a fresh full budget check. To deploy and seed both pools, inject `PORTALDOT_TEST_ACCOUNT_MNEMONIC`, set `PORTALDOT_P3_BROADCAST=1`, and run `npm run p2:deploy -- --resume --broadcast --acceptance-swap`. The broadcast path verifies the signing account against `PORTALDOT_TEST_ACCOUNT_ADDRESS`, and it can resume from a saved report.

The local P2 unit tests do not prove cross-contract rollback; that requires a contracts-runtime end-to-end run. Live deployment and swap acceptance remain pending testnet funding and a broadcast run.
