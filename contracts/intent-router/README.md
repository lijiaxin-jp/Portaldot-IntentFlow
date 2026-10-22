# IntentRouter

The P2 Router accepts a structured `SwapIntent` and one or two `RouteStep`
values. It verifies the owner or authorized Solver, route hash and execution
commitment, asset continuity, pool allowlist, minimum output, maximum fee,
deadline, snapshot age, replay status, and pause state before invoking only
allowlisted pools. It uses an execution guard and explicit `REVERT` after
external token or pool calls fail. The route hash uses the same network, block,
pool, and asset encoding as `defi-engine`.

The Router cannot independently fetch historical block hashes through the
standard ink! environment API, so it binds the supplied block hash into the
Solver commitment and checks the snapshot age; it does not prove that hash is
canonical. Local unit tests do not prove cross-contract rollback. The contract
has not been deployed because the test account needs more funds for code and
contract storage deposits. The generated `.contract` bundles and deployable
Wasm files are written under `target/p2-contracts/`; the deployment tool records
the current read-only preflight in `evidence/p3/p2-deployment.json`.
