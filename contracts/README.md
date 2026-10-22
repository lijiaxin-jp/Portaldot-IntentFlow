# ink! Contracts

The P2 source lives in a standalone Cargo workspace. P0 demonstrated the target
runtime's upload, instantiate, call, and event lifecycle for the probe; a second
independent instance remains inconclusive because the available account balance
did not cover its storage deposit.

- `p2-interface`: shared PSP22, AMM, Router, intent, and route types, plus the
  SHA-256 fingerprint format used by the Rust Solver.
- `psp22-test-token`: test-only PSP22 with admin minting.
- `amm-pool`: checked constant-product quotes, balanced liquidity deposits,
  proportional withdrawals, and exact-input swaps.
- `intent-router`: one/two-hop allowlisted settlement with Solver authorization,
  replay protection, pause, deadlines, fee/slippage checks, and explicit reverts
  after external calls.

Local tests cover token accounting and policy/math helpers. They do not execute
contracts against one another, so they do not prove cross-contract rollback.
Run that verification in a contracts runtime before treating P2 as fully
accepted. `RouteRejected` is not emitted on a reverted transaction because
events from reverted calls are rolled back; failures remain visible as call or
extrinsic errors.

`cargo test --manifest-path contracts/Cargo.toml --workspace --locked` runs the
local suite. `scripts/build-p2-contracts.ps1` uses the pinned Linux toolchain in
Docker on Windows, or requires `cargo-contract 4.1.3` and the Wasm target on
other hosts. It builds optimized, Portaldot-validated Wasm artifacts and ABI
bundles under `target/p2-contracts/`. Metadata and testnet addresses are not
checked in.

`scripts/deploy-p2-testnet.mjs` performs a no-broadcast deploy preflight by
default. A deployment plan pins the expected endpoint host and test-token seed
amounts. Broadcasting requires both `--broadcast` and
`PORTALDOT_P3_BROADCAST=1`, with the signer address and mnemonic supplied only
through `PORTALDOT_TEST_ACCOUNT_ADDRESS` and
`PORTALDOT_TEST_ACCOUNT_MNEMONIC`. P3 remains pending funding, deployment, and
finalized chain evidence.

If a broadcast stops after recording finalized steps, rerun with `--resume` and
the same plan, endpoint, and account. The script verifies the saved chain
identity and deployment inputs, confirms existing contract code hashes, and
skips deployment or initialization steps with finalized receipts. Broadcasting
still requires both explicit broadcast switches and the injected mnemonic.
