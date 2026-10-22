# Constant-Product AMM Pool

The P2 pool is a small testnet-only constant-product pool. It uses checked
512-bit intermediate arithmetic, requires canonical token ordering, and exposes
token pair, reserves, fee, LP share accounting, balanced liquidity changes, and
exact-input swaps. A swap accepts `amount_out_min` and a deadline and emits
`Swap`, `LiquidityAdded`, or `LiquidityRemoved`. It does not implement ticks,
flash loans, transferable LP shares, or arbitrary external calls. Cross-contract
token-call rollback still needs an end-to-end contracts-runtime test.
