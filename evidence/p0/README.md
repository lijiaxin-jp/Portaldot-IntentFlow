# P0 Testnet Evidence

## Result

The probe completed one finalized upload, instantiate, read, and event-emitting
call. A second run repeated the existing contract read and `ping` call, with
matching dry-run output and finalized events. The strict full-lifecycle
repeatability gate remains inconclusive because the account cannot fund a
second independent instance.

The testnet probe is separate from application data. IntentFlow datasets can
remain off-chain; this P0 only tests a small contract and does not require
application payloads to be stored on-chain.

## Root Cause

The earlier `Contracts.CodeRejected` result came from malformed SCALE input,
not an ink!/node Wasm compatibility failure. For a 5,000-byte Wasm vector, the
correct compact length is `0x214e`; the old request used `0x204e`. At block
`130248`, the malformed prefix returned `Contracts.CodeRejected`, while the
correct prefix succeeded for the same Wasm, origin, and block.

The official [Contracts errors](https://portaldot-dev.readthedocs.io/en/latest/module-interface/errors/contracts.html)
page describes `CodeRejected` as Wasm validation failure and notes unsupported
host APIs as a common cause. That diagnosis did not apply to this request
because its encoded vector was invalid. The attached
`CannotAddSelfAsDelegateDependency` text is a documentation excerpt; the probe
does not call delegate-dependency APIs.

The node exposes `ContractsApi` runtime API version 2. Its `instantiate`
signature takes `Option<Weight>`, `Option<Balance>`, a `Code<Hash>` enum, input
bytes, and salt. The Contracts pallet's host-function API version is 4; these
are distinct version values. The official [Contracts RPC page](https://portaldot-dev.readthedocs.io/en/latest/module-interface/rpc/contracts.html)
directs clients from the deprecated RPCs to the runtime interface. Runtime
metadata advanced from version 14 to 16 while `spec_version` remained 3002.

## Finalized Lifecycle

The probe Wasm is 5,000 bytes with SHA-256
`2eb12ff2ca18063619943f96934fc7d6b40e03a8364a06795c6bb0d49e696271` and code
hash `0x07211a71bb0ea630a8d3e1171493c1c0348692487a19995db28a10602a01d085`.
It was built with ink! 5.1.1 and passes local Wasmi validation.

- Upload finalized in block `130752`, hash `0xf966bd05d12d437668e1c0ebad782d1c28bf79d8da106e4c86432745ebee20f0`; `CodeStored` reported the expected code hash.
- Instantiate finalized in block `130755`, hash `0x39777f55518f92249f177de66190497b0bf7a33b5768cd35444784c4a0acab1b`; the event address matched the dry-run prediction.
- The contract address is `5E3miyBUvG1SX6KTT8zU4tx2XXwprqLv43ZP3riMXPQ1uMyn`.
- `get_sequence` returned SCALE `Result::Ok(0)` bytes `0x0000000000`; a same-block repeat produced the same result hash.
- `ping` finalized in block `130828` and again in block `130847`. Both emitted `Contracts.ContractEmitted` and `Contracts.Called`, followed by `System.ExtrinsicSuccess`.

The upload deposit was `303.96 tPOT`; the instantiate dry-run estimated `98.908 tPOT`. At block `130855`, the account had `93.57270970086899 tPOT` free and `303.96 tPOT` reserved. A second distinct instance needs `98.908 tPOT`; a read-only dry-run with that explicit deposit limit returned `Contracts.StorageDepositNotEnoughFunds`, leaving a `5.33529029913101 tPOT` shortfall before fees. No second instance transaction was submitted.

## Evidence

- `lifecycle-run.json`: first upload, instantiate, read, and ping receipts.
- `lifecycle-repeat.json`: second existing-contract read and ping run.
- `transactions.json`: compact transaction and event index.
- `capability-report.json`: capability decision and remaining funding gate.

The capability report separates the latest chain observation at block `130855`
from upload preflight results. The retained successful upload dry-run response
belongs to block `130285`; `account.json` also records a later preflight block
reference (`130842`) and its pass status, but does not retain that run's raw
response bytes or metadata hash. The `130285` response is not attributed to
either later block.

The account address and signing material are not included in evidence. The
signing material was injected through an environment variable and was not
written to disk. The reports retain the deployed contract address and public
transaction hashes for verification.
