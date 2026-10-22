# Test-only PSP22

The P2 token implements the shared PSP22 interface, checked `u128` balances and
allowances, `Transfer` and `Approval` events, an explicit admin, and an always
true `test_only()` marker. Only the admin can mint; zero-address and zero-value
minting are rejected. This token is for testnet protocol verification, not
production assets.
