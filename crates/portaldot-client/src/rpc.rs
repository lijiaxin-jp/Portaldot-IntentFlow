use crate::{
    AdapterError, ChainAdapter, ContractCall, ContractCapabilities, ContractEventRecord,
    RuntimeInfo, SimulationResult, TransactionEventRecord, TransactionSigner, TransactionSubmitter,
    TxReceipt,
};
use async_trait::async_trait;
use defi_domain::{AccountId, Balance, BlockRef, ContractId, GasWeight};
use parity_scale_codec::Decode;
use sha2::{Digest, Sha256};
use sp_core::{Pair, sr25519};
use std::collections::VecDeque;
use std::time::Duration;
use subxt::config::DefaultExtrinsicParamsBuilder;
use subxt::config::RpcConfigFor;
use subxt::config::substrate::H256;
use subxt::dynamic::{self, At, Value};
use subxt::events::Phase;
use subxt::ext::scale_decode::DecodeAsType;
use subxt::ext::scale_value::ValueDef;
use subxt::rpcs::client::{ReconnectingRpcClient, RpcClient};
use subxt::rpcs::methods::{LegacyRpcMethods, legacy::BlockNumber};
use subxt::utils::{AccountId32, MultiSignature};
use subxt::{OnlineClient, PolkadotConfig};

type RpcMethods = LegacyRpcMethods<RpcConfigFor<PolkadotConfig>>;

#[derive(Clone)]
pub struct PortaldotClient {
    endpoint: String,
    api: OnlineClient<PolkadotConfig>,
    methods: RpcMethods,
}

impl PortaldotClient {
    pub async fn connect(endpoint: impl Into<String>) -> Result<Self, AdapterError> {
        let endpoint = endpoint.into();
        if !(endpoint.starts_with("wss://") || endpoint.starts_with("ws://")) {
            return Err(AdapterError::Transport(
                "Portaldot RPC endpoint must use ws:// or wss://".into(),
            ));
        }
        let reconnecting = ReconnectingRpcClient::builder()
            .build(&endpoint)
            .await
            .map_err(|error| AdapterError::Transport(error.to_string()))?;
        let rpc = RpcClient::new(reconnecting);
        let api = OnlineClient::<PolkadotConfig>::from_rpc_client(rpc.clone())
            .await
            .map_err(|error| AdapterError::Transport(error.to_string()))?;
        let methods = RpcMethods::new(rpc.clone());
        Ok(Self {
            endpoint,
            api,
            methods,
        })
    }

    pub async fn subscribe_finalized_blocks(&self) -> Result<FinalizedBlockStream, AdapterError> {
        let last = self.latest_block().await?;
        let blocks = self
            .api
            .stream_blocks()
            .await
            .map_err(|error| AdapterError::Transport(error.to_string()))?;
        Ok(FinalizedBlockStream {
            endpoint: self.endpoint.clone(),
            client: self.clone(),
            blocks,
            last: Some(last),
            pending: VecDeque::new(),
        })
    }

    async fn account_balance_at(&self, account: AccountId) -> Result<Balance, AdapterError> {
        #[derive(DecodeAsType)]
        #[decode_as_type(crate_path = "subxt::ext::scale_decode")]
        struct AccountInfo {
            data: AccountData,
        }
        #[derive(DecodeAsType)]
        #[decode_as_type(crate_path = "subxt::ext::scale_decode")]
        struct AccountData {
            free: u128,
        }

        let at = self
            .api
            .at_current_block()
            .await
            .map_err(|error| AdapterError::Metadata(error.to_string()))?;
        let address = dynamic::storage::<([u8; 32],), AccountInfo>("System", "Account");
        let value = at
            .storage()
            .fetch(address, (account.0,))
            .await
            .map_err(|error| AdapterError::Metadata(error.to_string()))?;
        let info = value
            .decode()
            .map_err(|error| AdapterError::Metadata(error.to_string()))?;
        Ok(info.data.free)
    }

    async fn contract_call_at(
        &self,
        call: &ContractCall,
        block: BlockRef,
    ) -> Result<SimulationResult, AdapterError> {
        let at = self
            .api
            .at_block(H256::from(block.hash))
            .await
            .map_err(|error| AdapterError::Metadata(error.to_string()))?;
        if at.block_number() != block.number {
            return Err(AdapterError::Metadata(
                "requested block hash and number do not match".into(),
            ));
        }

        let mut input = Vec::with_capacity(call.input.len() + call.selector.len());
        input.extend_from_slice(&call.selector);
        input.extend_from_slice(&call.input);
        let origin = AccountId32::from(call.origin.0);
        let destination = AccountId32::from(call.contract.0);
        let payload = dynamic::runtime_api_call::<_, Value>(
            "ContractsApi",
            "call",
            (
                origin,
                destination,
                call.attached_value,
                None::<Value>,
                None::<Value>,
                input,
            ),
        );
        let result = at
            .runtime_apis()
            .call(payload)
            .await
            .map_err(|error| AdapterError::Contract(error.to_string()))?;
        simulation_from_runtime_value(result)
    }

    async fn runtime_capabilities(&self) -> Result<ContractCapabilities, AdapterError> {
        let at = self
            .api
            .at_current_block()
            .await
            .map_err(|error| AdapterError::Metadata(error.to_string()))?;
        let metadata = at.metadata();
        let contracts = metadata.pallet_by_name("Contracts");
        let contracts_api = metadata.runtime_api_trait_by_name("ContractsApi");
        let has_runtime_method = |name: &str| {
            contracts_api
                .as_ref()
                .is_some_and(|api| api.method_by_name(name).is_some())
        };
        let has_call = |name: &str| {
            contracts
                .as_ref()
                .and_then(|pallet| pallet.call_variant_by_name(name))
                .is_some()
        };
        let event_queries = contracts
            .as_ref()
            .and_then(|pallet| pallet.event_variants())
            .is_some_and(|events| events.iter().any(|event| event.name == "ContractEmitted"));
        Ok(ContractCapabilities {
            contracts_module: contracts.is_some(),
            dry_run: has_runtime_method("call"),
            upload: has_runtime_method("upload_code") || has_call("upload_code"),
            instantiate: has_runtime_method("instantiate_with_code")
                || has_runtime_method("instantiate")
                || has_call("instantiate_with_code"),
            event_queries,
        })
    }
}

pub struct EnvironmentSr25519Signer {
    pair: sr25519::Pair,
    account: AccountId,
}

impl EnvironmentSr25519Signer {
    pub fn from_env() -> Result<Self, AdapterError> {
        use zeroize::Zeroize;

        let mut secret_uri = std::env::var("PORTALDOT_SIGNER_URI")
            .map_err(|_| AdapterError::Rejected("PORTALDOT_SIGNER_URI is not set".into()))?;
        let pair = sr25519::Pair::from_string(&secret_uri, None)
            .map_err(|_| AdapterError::Rejected("PORTALDOT_SIGNER_URI is invalid".into()));
        secret_uri.zeroize();
        let pair = pair?;
        let account = AccountId(pair.public().0);
        Ok(Self { pair, account })
    }
}

impl TransactionSigner for EnvironmentSr25519Signer {
    fn account_id(&self) -> AccountId {
        self.account
    }

    fn sign_payload(&self, payload: &[u8]) -> Result<MultiSignature, AdapterError> {
        Ok(MultiSignature::Sr25519(self.pair.sign(payload).0))
    }
}

#[async_trait]
impl TransactionSubmitter for PortaldotClient {
    async fn submit(
        &self,
        call: ContractCall,
        gas_limit: GasWeight,
        signer: &dyn TransactionSigner,
    ) -> Result<TxReceipt, AdapterError> {
        if signer.account_id() != call.origin {
            return Err(AdapterError::Rejected(
                "transaction signer does not match the call origin".into(),
            ));
        }

        let mut input = Vec::with_capacity(call.selector.len() + call.input.len());
        input.extend_from_slice(&call.selector);
        input.extend_from_slice(&call.input);
        let gas_limit = Value::named_composite([
            ("ref_time", Value::u128(gas_limit.ref_time.into())),
            ("proof_size", Value::u128(gas_limit.proof_size.into())),
        ]);
        let payload = dynamic::tx(
            "Contracts",
            "call",
            (
                Value::from_bytes(call.contract.0),
                call.attached_value,
                gas_limit,
                None::<Value>,
                Value::from_bytes(input),
            ),
        );
        let account = AccountId32::from(call.origin.0);
        let transactions = self
            .api
            .tx()
            .await
            .map_err(|error| AdapterError::Transport(error.to_string()))?;
        let params = DefaultExtrinsicParamsBuilder::<PolkadotConfig>::new().build();
        let mut signable = transactions
            .create_signable(&payload, &account, params)
            .await
            .map_err(|error| AdapterError::Metadata(error.to_string()))?;
        let signature = signer.sign_payload(
            &signable
                .signer_payload()
                .map_err(|error| AdapterError::Metadata(error.to_string()))?,
        )?;
        let signed = signable
            .sign_with_account_and_signature(&account, &signature)
            .map_err(|error| AdapterError::Metadata(error.to_string()))?;
        let progress = signed
            .submit_and_watch()
            .await
            .map_err(|error| AdapterError::Rejected(error.to_string()))?;
        let in_block = progress
            .wait_for_finalized()
            .await
            .map_err(|error| AdapterError::Rejected(error.to_string()))?;
        let tx_hash = in_block.extrinsic_hash().0;
        let block_hash = in_block.block_hash();
        let at = self
            .api
            .at_block(block_hash)
            .await
            .map_err(|error| AdapterError::Metadata(error.to_string()))?;
        let block = block_ref(at.block_number(), block_hash)?;
        let extrinsic_events = in_block
            .fetch_events()
            .await
            .map_err(|error| AdapterError::Metadata(error.to_string()))?;
        let extrinsic_index = u32::try_from(extrinsic_events.extrinsic_index())
            .map_err(|_| AdapterError::Metadata("extrinsic index exceeds u32".into()))?;
        let mut success = false;
        let mut failure_reason = None;
        let mut event_records = Vec::new();
        for event in extrinsic_events.iter() {
            let event = event.map_err(|error| AdapterError::Metadata(error.to_string()))?;
            let pallet = event.pallet_name().to_owned();
            let event_name = event.event_name().to_owned();
            let data = event.field_bytes();
            if pallet == "System" && event_name == "ExtrinsicSuccess" {
                success = true;
            } else if pallet == "System" && event_name == "ExtrinsicFailed" {
                failure_reason = Some(format!("System::ExtrinsicFailed: 0x{}", hex::encode(data)));
            }
            let contract = if pallet == "Contracts" && event_name == "ContractEmitted" {
                let mut fields = data;
                <[u8; 32]>::decode(&mut fields).ok().map(ContractId)
            } else {
                None
            };
            event_records.push(TransactionEventRecord {
                pallet,
                event: event_name,
                block,
                extrinsic_index,
                event_index: event.index(),
                contract,
                topics: event.topics().iter().map(|topic| topic.0).collect(),
                data: data.to_vec(),
            });
        }
        Ok(TxReceipt {
            tx_hash,
            block,
            extrinsic_index,
            success,
            gas_used: GasWeight::default(),
            events: event_records
                .iter()
                .map(|event| event.data.clone())
                .collect(),
            event_records,
            failure_reason,
        })
    }
}

#[async_trait]
impl ChainAdapter for PortaldotClient {
    async fn latest_block(&self) -> Result<BlockRef, AdapterError> {
        let at = self
            .api
            .at_current_block()
            .await
            .map_err(|error| AdapterError::Transport(error.to_string()))?;
        block_ref(at.block_number(), at.block_hash())
    }

    async fn runtime_info(&self) -> Result<RuntimeInfo, AdapterError> {
        let at = self
            .api
            .at_current_block()
            .await
            .map_err(|error| AdapterError::Transport(error.to_string()))?;
        let hash = at.block_hash();
        let runtime = self
            .methods
            .state_get_runtime_version(Some(hash))
            .await
            .map_err(|error| AdapterError::Transport(error.to_string()))?;
        let metadata = self
            .methods
            .state_get_metadata(Some(hash))
            .await
            .map_err(|error| AdapterError::Transport(error.to_string()))?
            .into_raw();
        let spec_name = runtime
            .other
            .get("specName")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| AdapterError::Metadata("runtime specName is missing".into()))?
            .to_owned();
        Ok(RuntimeInfo {
            spec_name,
            spec_version: runtime.spec_version,
            transaction_version: runtime.transaction_version,
            metadata_hash: Sha256::digest(metadata).into(),
        })
    }

    async fn probe_capabilities(&self) -> Result<ContractCapabilities, AdapterError> {
        self.runtime_capabilities().await
    }

    async fn account_balance(&self, account: AccountId) -> Result<Balance, AdapterError> {
        self.account_balance_at(account).await
    }

    async fn dry_run_at(
        &self,
        call: ContractCall,
        block: BlockRef,
    ) -> Result<SimulationResult, AdapterError> {
        self.contract_call_at(&call, block).await
    }

    async fn events(&self, block: BlockRef) -> Result<Vec<ContractEventRecord>, AdapterError> {
        let at = self
            .api
            .at_block(H256::from(block.hash))
            .await
            .map_err(|error| AdapterError::Metadata(error.to_string()))?;
        if at.block_number() != block.number {
            return Err(AdapterError::Metadata(
                "requested event block hash and number do not match".into(),
            ));
        }
        let events = at
            .events()
            .fetch()
            .await
            .map_err(|error| AdapterError::Metadata(error.to_string()))?;
        let mut decoded = Vec::new();
        for event in events.iter() {
            let event = event.map_err(|error| AdapterError::Metadata(error.to_string()))?;
            if event.pallet_name() != "Contracts" || event.event_name() != "ContractEmitted" {
                continue;
            }
            let mut fields = event.field_bytes();
            let contract = <[u8; 32]>::decode(&mut fields)
                .map_err(|error| AdapterError::Metadata(error.to_string()))?;
            let data = Vec::<u8>::decode(&mut fields)
                .map_err(|error| AdapterError::Metadata(error.to_string()))?;
            if !fields.is_empty() {
                return Err(AdapterError::Metadata(
                    "Contracts::ContractEmitted has unexpected trailing event fields".into(),
                ));
            }
            let extrinsic_index = match event.phase() {
                Phase::ApplyExtrinsic(index) => Some(index),
                Phase::Finalization | Phase::Initialization => None,
            };
            decoded.push(ContractEventRecord {
                contract: ContractId(contract),
                block,
                extrinsic_index,
                event_index: event.index(),
                topics: event.topics().iter().map(|topic| topic.0).collect(),
                data,
            });
        }
        Ok(decoded)
    }
}

pub struct FinalizedBlockStream {
    endpoint: String,
    client: PortaldotClient,
    blocks: subxt::client::Blocks<PolkadotConfig>,
    last: Option<BlockRef>,
    pending: VecDeque<BlockRef>,
}

impl FinalizedBlockStream {
    pub async fn next_block(&mut self) -> Result<BlockRef, AdapterError> {
        loop {
            if let Some(block) = self.pending.pop_front() {
                self.last = Some(block);
                return Ok(block);
            }
            let Some(next) = self.blocks.next().await else {
                self.reconnect().await?;
                continue;
            };
            let block = match next {
                Ok(block) => block_ref(block.number(), block.hash())?,
                Err(error) => {
                    tracing::warn!(error = %error, "finalized block stream disconnected; reconnecting");
                    self.reconnect().await?;
                    continue;
                }
            };
            let Some(last) = self.last else {
                self.pending.push_back(block);
                continue;
            };
            if block.number < last.number {
                continue;
            }
            if block.number == last.number {
                if block.hash != last.hash {
                    return Err(AdapterError::Transport(
                        "finalized block hash changed at an already observed height".into(),
                    ));
                }
                continue;
            }
            let next_number = last
                .number
                .checked_add(1)
                .ok_or_else(|| AdapterError::Metadata("block number overflow".into()))?;
            if block.number > next_number {
                self.backfill(next_number, block).await?;
            } else {
                self.pending.push_back(block);
            }
        }
    }

    async fn reconnect(&mut self) -> Result<(), AdapterError> {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let client = PortaldotClient::connect(self.endpoint.clone()).await?;
        let blocks = client
            .api
            .stream_blocks()
            .await
            .map_err(|error| AdapterError::Transport(error.to_string()))?;
        self.client = client;
        self.blocks = blocks;
        Ok(())
    }

    async fn backfill(&mut self, mut number: u64, latest: BlockRef) -> Result<(), AdapterError> {
        while number < latest.number {
            let hash = self
                .client
                .methods
                .chain_get_block_hash(Some(BlockNumber::from(number)))
                .await
                .map_err(|error| AdapterError::Transport(error.to_string()))?
                .ok_or_else(|| {
                    AdapterError::Transport(format!("finalized block {number} is unavailable"))
                })?;
            let header = self
                .client
                .methods
                .chain_get_header(Some(hash))
                .await
                .map_err(|error| AdapterError::Transport(error.to_string()))?
                .ok_or_else(|| {
                    AdapterError::Transport(format!("finalized block {number} has no header"))
                })?;
            if header.number != number {
                return Err(AdapterError::Metadata(
                    "block-number lookup returned a mismatched header".into(),
                ));
            }
            self.pending.push_back(block_ref(number, hash)?);
            number = number
                .checked_add(1)
                .ok_or_else(|| AdapterError::Metadata("block number overflow".into()))?;
        }
        self.pending.push_back(latest);
        Ok(())
    }
}

fn block_ref(number: u64, hash: H256) -> Result<BlockRef, AdapterError> {
    BlockRef::new(number, hash.0).map_err(|error| AdapterError::Metadata(error.to_string()))
}

fn simulation_from_runtime_value(value: Value) -> Result<SimulationResult, AdapterError> {
    let result = match variant(&value) {
        Some(("Err", reason)) if field(&value, &["gas_required", "gasRequired"]).is_none() => {
            return Ok(SimulationResult {
                success: false,
                gas_required: GasWeight::default(),
                output: Vec::new(),
                events: Vec::new(),
                failure_reason: Some(format!("ContractsApi::call rejected: {reason:?}")),
            });
        }
        Some(("Ok", Some(inner))) if field(inner, &["gas_required", "gasRequired"]).is_some() => {
            inner
        }
        _ => &value,
    };
    let gas = field(result, &["gas_required", "gasRequired"]).ok_or_else(|| {
        AdapterError::Metadata("ContractsApi::call response has no gas_required field".into())
    })?;
    let gas_required = GasWeight {
        ref_time: field(gas, &["ref_time", "refTime"])
            .and_then(Value::as_u128)
            .and_then(|value| u64::try_from(value).ok())
            .ok_or_else(|| AdapterError::Metadata("gas ref_time is invalid".into()))?,
        proof_size: field(gas, &["proof_size", "proofSize"])
            .and_then(Value::as_u128)
            .and_then(|value| u64::try_from(value).ok())
            .ok_or_else(|| AdapterError::Metadata("gas proof_size is invalid".into()))?,
    };
    let contract_result = field(result, &["result"]);
    if let Some(contract_result) = contract_result {
        if let Some(("Err", reason)) = variant(contract_result) {
            return Ok(SimulationResult {
                success: false,
                gas_required,
                output: Vec::new(),
                events: Vec::new(),
                failure_reason: Some(format!("contract dry-run reverted: {reason:?}")),
            });
        }
    }
    let return_value =
        match contract_result.and_then(|value| variant(value).map(|(_, payload)| payload)) {
            Some(Some(value)) => value,
            _ => contract_result.unwrap_or(result),
        };
    if field(return_value, &["flags"])
        .and_then(Value::as_u128)
        .is_some_and(|flags| flags & 1 != 0)
    {
        return Ok(SimulationResult {
            success: false,
            gas_required,
            output: Vec::new(),
            events: Vec::new(),
            failure_reason: Some("contract execution returned the REVERT flag".into()),
        });
    }
    let events = event_data(result)?;
    let output = field(return_value, &["data"])
        .and_then(value_bytes)
        .or_else(|| field(result, &["data"]).and_then(value_bytes))
        .ok_or_else(|| {
            AdapterError::Metadata("ContractsApi::call response has no output data".into())
        })?;
    Ok(SimulationResult {
        success: true,
        gas_required,
        output,
        events,
        failure_reason: None,
    })
}

fn event_data(result: &Value) -> Result<Vec<Vec<u8>>, AdapterError> {
    let Some(events) = field(result, &["events"]) else {
        return Ok(Vec::new());
    };
    let ValueDef::Composite(events) = &events.value else {
        return Err(AdapterError::Metadata(
            "ContractsApi::call response events are not a sequence".into(),
        ));
    };
    events
        .values()
        .map(|event| {
            field(event, &["data"])
                .and_then(value_bytes)
                .ok_or_else(|| {
                    AdapterError::Metadata(
                        "ContractsApi::call response contains an invalid event payload".into(),
                    )
                })
        })
        .collect()
}

fn variant(value: &Value) -> Option<(&str, Option<&Value>)> {
    match &value.value {
        ValueDef::Variant(variant) => Some((variant.name.as_str(), variant.values.values().next())),
        _ => None,
    }
}

fn field<'a>(value: &'a Value, names: &[&str]) -> Option<&'a Value> {
    names.iter().find_map(|name| value.at(*name))
}

fn value_bytes(value: &Value) -> Option<Vec<u8>> {
    let composite = match &value.value {
        ValueDef::Composite(composite) => composite,
        _ => return None,
    };
    composite
        .values()
        .map(|byte| byte.as_u128().and_then(|value| u8::try_from(value).ok()))
        .collect()
}
