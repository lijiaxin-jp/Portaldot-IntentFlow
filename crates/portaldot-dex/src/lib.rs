//! Portaldot pool adapter contracts.
//!
//! No exchange-specific adapter is imported here. A concrete adapter must
//! decode the ink! pool metadata and events after P0 confirms the deployed
//! contract ABI.

use async_trait::async_trait;
use defi_domain::{
    AccountId, AssetId, Balance, BlockRef, ContractId, Hash, PoolKind, PoolSnapshot,
};
use parity_scale_codec::{Decode, Encode};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, path::Path, sync::Arc};
use thiserror::Error;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PoolDescriptor {
    pub pool_id: ContractId,
    pub kind: PoolKind,
    pub asset_0: AssetId,
    pub asset_1: AssetId,
    pub fee_bps: u32,
    pub code_hash: Option<Hash>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum PoolEventData {
    LiquidityAdded {
        amount_0: Balance,
        amount_1: Balance,
        shares: Balance,
    },
    LiquidityRemoved {
        amount_0: Balance,
        amount_1: Balance,
        shares: Balance,
    },
    Swap {
        asset_in: AccountId,
        asset_out: AccountId,
        amount_in: Balance,
        amount_out: Balance,
        fee_paid: Balance,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PoolEvent {
    pub pool_id: ContractId,
    pub block: BlockRef,
    pub event_kind: String,
    pub topics: Vec<Hash>,
    pub payload: Vec<u8>,
    pub data: PoolEventData,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum PoolAdapterError {
    #[error("pool metadata is invalid: {0}")]
    InvalidMetadata(String),
    #[error("pool read failed: {0}")]
    Read(String),
    #[error("chain query failed: {0}")]
    Chain(#[from] portaldot_client::AdapterError),
    #[error("event decode failed: {0}")]
    EventDecode(String),
}

#[async_trait]
pub trait PoolAdapter: Send + Sync {
    async fn describe(&self, pool: ContractId) -> Result<PoolDescriptor, PoolAdapterError>;
    async fn snapshot(&self, pool: ContractId) -> Result<PoolSnapshot, PoolAdapterError>;
    async fn quote(
        &self,
        pool: ContractId,
        asset_in: AssetId,
        amount_in: Balance,
    ) -> Result<Balance, PoolAdapterError>;
    fn decode_event(
        &self,
        pool: ContractId,
        event: &portaldot_client::ContractEventRecord,
    ) -> Result<PoolEvent, PoolAdapterError>;
}

#[derive(Clone, Debug)]
pub struct InkAmmPoolAdapter<C> {
    chain: Arc<C>,
    origin: AccountId,
    abi: AmmPoolAbi,
}

#[derive(Clone, Debug)]
struct AmmPoolAbi {
    methods: BTreeMap<String, [u8; 4]>,
    events: BTreeMap<Hash, String>,
}

#[derive(Deserialize)]
struct ContractBundle {
    spec: ContractSpec,
}

#[derive(Deserialize)]
struct ContractSpec {
    messages: Vec<ContractMessage>,
    events: Vec<ContractEvent>,
}

#[derive(Deserialize)]
struct ContractMessage {
    label: String,
    selector: String,
}

#[derive(Deserialize)]
struct ContractEvent {
    label: String,
    #[serde(rename = "signature_topic")]
    signature_topic: String,
}

impl AmmPoolAbi {
    fn from_bundle(path: impl AsRef<Path>) -> Result<Self, PoolAdapterError> {
        let source = fs::read_to_string(path)
            .map_err(|error| PoolAdapterError::InvalidMetadata(error.to_string()))?;
        let bundle: ContractBundle = serde_json::from_str(&source)
            .map_err(|error| PoolAdapterError::InvalidMetadata(error.to_string()))?;
        let mut methods = BTreeMap::new();
        for message in bundle.spec.messages {
            let name = message.label.rsplit("::").next().unwrap_or(&message.label);
            methods.insert(name.to_owned(), decode_fixed_hex(&message.selector)?);
        }
        let mut events = BTreeMap::new();
        for event in bundle.spec.events {
            events.insert(decode_fixed_hex(&event.signature_topic)?, event.label);
        }
        for method in [
            "token_0",
            "token_1",
            "fee_bps",
            "reserves",
            "quote_exact_input",
        ] {
            if !methods.contains_key(method) {
                return Err(PoolAdapterError::InvalidMetadata(format!(
                    "AMM pool ABI is missing `{method}`"
                )));
            }
        }
        for event in ["LiquidityAdded", "LiquidityRemoved", "Swap"] {
            if !events.values().any(|name| name == event) {
                return Err(PoolAdapterError::InvalidMetadata(format!(
                    "AMM pool ABI is missing `{event}`"
                )));
            }
        }
        Ok(Self { methods, events })
    }

    fn selector(&self, name: &str) -> Result<[u8; 4], PoolAdapterError> {
        self.methods.get(name).copied().ok_or_else(|| {
            PoolAdapterError::InvalidMetadata(format!("ABI has no `{name}` message"))
        })
    }
}

impl<C> InkAmmPoolAdapter<C> {
    pub fn from_contract_bundle(
        chain: Arc<C>,
        origin: AccountId,
        bundle_path: impl AsRef<Path>,
    ) -> Result<Self, PoolAdapterError> {
        Ok(Self {
            chain,
            origin,
            abi: AmmPoolAbi::from_bundle(bundle_path)?,
        })
    }
}

impl<C: portaldot_client::ChainAdapter> InkAmmPoolAdapter<C> {
    async fn query<T: Decode>(
        &self,
        pool: ContractId,
        block: BlockRef,
        message: &str,
        input: Vec<u8>,
    ) -> Result<T, PoolAdapterError> {
        let simulation = self
            .chain
            .dry_run_at(
                portaldot_client::ContractCall {
                    origin: self.origin,
                    contract: pool,
                    selector: self.abi.selector(message)?,
                    input,
                    attached_value: 0,
                },
                block,
            )
            .await?;
        if !simulation.success {
            return Err(PoolAdapterError::Read(
                simulation
                    .failure_reason
                    .unwrap_or_else(|| format!("{message} dry-run failed")),
            ));
        }
        let mut bytes = simulation.output.as_slice();
        let decoded =
            T::decode(&mut bytes).map_err(|error| PoolAdapterError::Read(error.to_string()))?;
        if !bytes.is_empty() {
            return Err(PoolAdapterError::Read(format!(
                "{message} returned trailing SCALE bytes"
            )));
        }
        Ok(decoded)
    }

    async fn describe_at(
        &self,
        pool: ContractId,
        block: BlockRef,
    ) -> Result<PoolDescriptor, PoolAdapterError> {
        let token_0: [u8; 32] = self.query(pool, block, "token_0", Vec::new()).await?;
        let token_1: [u8; 32] = self.query(pool, block, "token_1", Vec::new()).await?;
        let fee_bps: u32 = self.query(pool, block, "fee_bps", Vec::new()).await?;
        let asset_0 = AssetId::Contract(ContractId(token_0));
        let asset_1 = AssetId::Contract(ContractId(token_1));
        if asset_0 >= asset_1 {
            return Err(PoolAdapterError::InvalidMetadata(
                "pool token addresses are not in canonical order".into(),
            ));
        }
        Ok(PoolDescriptor {
            pool_id: pool,
            kind: PoolKind::ConstantProduct,
            asset_0,
            asset_1,
            fee_bps,
            code_hash: None,
        })
    }
}

#[async_trait]
impl<C: portaldot_client::ChainAdapter + 'static> PoolAdapter for InkAmmPoolAdapter<C> {
    async fn describe(&self, pool: ContractId) -> Result<PoolDescriptor, PoolAdapterError> {
        self.describe_at(pool, self.chain.latest_block().await?)
            .await
    }

    async fn snapshot(&self, pool: ContractId) -> Result<PoolSnapshot, PoolAdapterError> {
        let block = self.chain.latest_block().await?;
        let descriptor = self.describe_at(pool, block).await?;
        let (reserve_0, reserve_1): (Balance, Balance) =
            self.query(pool, block, "reserves", Vec::new()).await?;
        let snapshot = PoolSnapshot {
            pool_id: pool,
            kind: descriptor.kind,
            asset_0: descriptor.asset_0,
            asset_1: descriptor.asset_1,
            reserve_0,
            reserve_1,
            fee_bps: descriptor.fee_bps,
            observed_at: block,
            code_hash: descriptor.code_hash,
        };
        snapshot
            .validate()
            .map_err(|error| PoolAdapterError::InvalidMetadata(error.to_string()))?;
        Ok(snapshot)
    }

    async fn quote(
        &self,
        pool: ContractId,
        asset_in: AssetId,
        amount_in: Balance,
    ) -> Result<Balance, PoolAdapterError> {
        let AssetId::Contract(asset_in) = asset_in else {
            return Err(PoolAdapterError::InvalidMetadata(
                "AMM pool only accepts contract assets".into(),
            ));
        };
        let block = self.chain.latest_block().await?;
        let mut input = asset_in.0.encode();
        input.extend_from_slice(&amount_in.encode());
        let result: Result<Balance, u8> =
            self.query(pool, block, "quote_exact_input", input).await?;
        result.map_err(|error| {
            PoolAdapterError::Read(format!(
                "quote_exact_input reverted: {}",
                contract_error_name(error)
            ))
        })
    }

    fn decode_event(
        &self,
        pool: ContractId,
        event: &portaldot_client::ContractEventRecord,
    ) -> Result<PoolEvent, PoolAdapterError> {
        if event.contract != pool {
            return Err(PoolAdapterError::EventDecode(
                "event belongs to a different contract".into(),
            ));
        }
        let signature = event.topics.first().ok_or_else(|| {
            PoolAdapterError::EventDecode("contract event has no signature topic".into())
        })?;
        let event_kind = self
            .abi
            .events
            .get(signature)
            .ok_or_else(|| PoolAdapterError::EventDecode("unknown AMM event topic".into()))?
            .clone();
        let data = match event_kind.as_str() {
            "LiquidityAdded" => {
                let (amount_0, amount_1, shares) = decode_event_payload(&event.data)?;
                PoolEventData::LiquidityAdded {
                    amount_0,
                    amount_1,
                    shares,
                }
            }
            "LiquidityRemoved" => {
                let (amount_0, amount_1, shares) = decode_event_payload(&event.data)?;
                PoolEventData::LiquidityRemoved {
                    amount_0,
                    amount_1,
                    shares,
                }
            }
            "Swap" => {
                let (asset_in, asset_out, amount_in, amount_out, fee_paid) =
                    decode_event_payload(&event.data)?;
                PoolEventData::Swap {
                    asset_in: AccountId(asset_in),
                    asset_out: AccountId(asset_out),
                    amount_in,
                    amount_out,
                    fee_paid,
                }
            }
            _ => {
                return Err(PoolAdapterError::EventDecode(format!(
                    "unsupported AMM event `{event_kind}`"
                )));
            }
        };
        Ok(PoolEvent {
            pool_id: pool,
            block: event.block,
            event_kind,
            topics: event.topics.clone(),
            payload: event.data.clone(),
            data,
        })
    }
}

fn decode_event_payload<T: Decode>(bytes: &[u8]) -> Result<T, PoolAdapterError> {
    let mut remaining = bytes;
    let decoded = T::decode(&mut remaining)
        .map_err(|error| PoolAdapterError::EventDecode(error.to_string()))?;
    if !remaining.is_empty() {
        return Err(PoolAdapterError::EventDecode(
            "event payload contains trailing SCALE bytes".into(),
        ));
    }
    Ok(decoded)
}

fn contract_error_name(index: u8) -> &'static str {
    match index {
        0 => "Unauthorized",
        1 => "ZeroAddress",
        2 => "InvalidAmount",
        3 => "InsufficientBalance",
        4 => "InsufficientAllowance",
        5 => "ArithmeticOverflow",
        6 => "InvalidAssetPair",
        7 => "InvalidFee",
        8 => "InvalidDeadline",
        9 => "InvalidRoute",
        10 => "PoolNotAllowed",
        11 => "Paused",
        12 => "SlippageExceeded",
        13 => "FeeLimitExceeded",
        14 => "TokenCallFailed",
        15 => "PoolCallFailed",
        16 => "InsufficientLiquidity",
        17 => "InsufficientShares",
        18 => "UnbalancedLiquidity",
        19 => "StaleRoute",
        20 => "ReentrantCall",
        _ => "UnknownContractError",
    }
}

fn decode_fixed_hex<const N: usize>(value: &str) -> Result<[u8; N], PoolAdapterError> {
    let value = value.strip_prefix("0x").unwrap_or(value);
    let bytes =
        hex::decode(value).map_err(|error| PoolAdapterError::InvalidMetadata(error.to_string()))?;
    bytes.try_into().map_err(|bytes: Vec<u8>| {
        PoolAdapterError::InvalidMetadata(format!(
            "ABI value has {} bytes; expected {N}",
            bytes.len()
        ))
    })
}
