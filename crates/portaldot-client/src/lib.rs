//! Portaldot chain boundary.
//!
//! This crate defines the business-level operations required by IntentFlow.
//! The concrete WebSocket and metadata implementation is intentionally left
//! behind this trait until P0 confirms the testnet runtime capabilities.

use async_trait::async_trait;
use defi_domain::{AccountId, Balance, BlockRef, ContractId, Hash, PoolSnapshot};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimeInfo {
    pub spec_name: String,
    pub spec_version: u32,
    pub transaction_version: u32,
    pub metadata_hash: Hash,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ContractCapabilities {
    pub contracts_module: bool,
    pub dry_run: bool,
    pub upload: bool,
    pub instantiate: bool,
    pub event_queries: bool,
}

impl ContractCapabilities {
    #[must_use]
    pub const fn ink_ready(self) -> bool {
        self.contracts_module
            && self.dry_run
            && self.upload
            && self.instantiate
            && self.event_queries
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CapabilityReport {
    pub network: String,
    pub latest_block: BlockRef,
    pub runtime: RuntimeInfo,
    pub capabilities: ContractCapabilities,
    pub account: AccountId,
    pub account_balance: Balance,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ContractCall {
    pub contract: ContractId,
    pub selector: [u8; 4],
    pub input: Vec<u8>,
    pub attached_value: Balance,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SignedCall {
    pub call: ContractCall,
    pub signer: AccountId,
    pub signed_payload: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SimulationResult {
    pub success: bool,
    pub gas_required: Balance,
    pub output: Vec<u8>,
    pub events: Vec<Vec<u8>>,
    pub failure_reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TxReceipt {
    pub tx_hash: Hash,
    pub block: BlockRef,
    pub success: bool,
    pub gas_used: Balance,
    pub events: Vec<Vec<u8>>,
    pub failure_reason: Option<String>,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum AdapterError {
    #[error("transport error: {0}")]
    Transport(String),
    #[error("runtime capability is unavailable: {0}")]
    Capability(String),
    #[error("metadata decoding failed: {0}")]
    Metadata(String),
    #[error("contract call failed: {0}")]
    Contract(String),
    #[error("transaction was rejected: {0}")]
    Rejected(String),
}

#[async_trait]
pub trait ChainAdapter: Send + Sync {
    async fn latest_block(&self) -> Result<BlockRef, AdapterError>;
    async fn runtime_info(&self) -> Result<RuntimeInfo, AdapterError>;
    async fn probe_capabilities(&self) -> Result<ContractCapabilities, AdapterError>;
    async fn account_balance(&self, account: AccountId) -> Result<Balance, AdapterError>;
    async fn read_pool(&self, pool: ContractId) -> Result<PoolSnapshot, AdapterError>;
    async fn dry_run(&self, call: ContractCall) -> Result<SimulationResult, AdapterError>;
    async fn submit(&self, call: SignedCall) -> Result<TxReceipt, AdapterError>;
    async fn events(&self, block: BlockRef) -> Result<Vec<Vec<u8>>, AdapterError>;
}

/// A capability check that can be used by scripts before creating contracts.
pub async fn probe<A: ChainAdapter>(
    adapter: &A,
    network: impl Into<String>,
    account: AccountId,
) -> Result<CapabilityReport, AdapterError> {
    let latest_block = adapter.latest_block().await?;
    let runtime = adapter.runtime_info().await?;
    let capabilities = adapter.probe_capabilities().await?;
    let account_balance = adapter.account_balance(account).await?;
    Ok(CapabilityReport {
        network: network.into(),
        latest_block,
        runtime,
        capabilities,
        account,
        account_balance,
    })
}
