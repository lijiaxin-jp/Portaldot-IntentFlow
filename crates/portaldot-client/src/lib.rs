//! Portaldot chain boundary.
//!
//! This crate defines IntentFlow's business-level chain operations and the
//! read-only Subxt implementation used by the Portaldot adapter.

use async_trait::async_trait;
use defi_domain::{AccountId, Balance, BlockRef, ContractId, GasWeight, Hash};
use serde::{Deserialize, Serialize};
use subxt::utils::MultiSignature;
use thiserror::Error;

mod rpc;
pub use rpc::{EnvironmentSr25519Signer, FinalizedBlockStream, PortaldotClient};

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
    pub const fn ink_ready(&self) -> bool {
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
    pub origin: AccountId,
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
    pub gas_required: GasWeight,
    pub output: Vec<u8>,
    pub events: Vec<Vec<u8>>,
    pub failure_reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TxReceipt {
    pub tx_hash: Hash,
    pub block: BlockRef,
    pub extrinsic_index: u32,
    pub success: bool,
    pub gas_used: GasWeight,
    pub events: Vec<Vec<u8>>,
    pub event_records: Vec<TransactionEventRecord>,
    pub failure_reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ContractEventRecord {
    pub contract: ContractId,
    pub block: BlockRef,
    pub extrinsic_index: Option<u32>,
    pub event_index: u32,
    pub topics: Vec<Hash>,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TransactionEventRecord {
    pub pallet: String,
    pub event: String,
    pub block: BlockRef,
    pub extrinsic_index: u32,
    pub event_index: u32,
    pub contract: Option<ContractId>,
    pub topics: Vec<Hash>,
    pub data: Vec<u8>,
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
    async fn dry_run_at(
        &self,
        call: ContractCall,
        block: BlockRef,
    ) -> Result<SimulationResult, AdapterError>;
    async fn events(&self, block: BlockRef) -> Result<Vec<ContractEventRecord>, AdapterError>;
}

pub trait TransactionSigner: Send + Sync {
    fn account_id(&self) -> AccountId;
    fn sign_payload(&self, payload: &[u8]) -> Result<MultiSignature, AdapterError>;
}

#[async_trait]
pub trait TransactionSubmitter: ChainAdapter {
    async fn submit(
        &self,
        call: ContractCall,
        gas_limit: GasWeight,
        signer: &dyn TransactionSigner,
    ) -> Result<TxReceipt, AdapterError>;
}

/// A capability check that can be used by scripts before creating contracts.
pub async fn probe<A: ChainAdapter>(
    adapter: &A,
    network: impl Into<String>,
    account: AccountId,
) -> Result<CapabilityReport, AdapterError> {
    let network = network.into();
    tracing::info!(network = %network, "starting Portaldot capability probe");
    let latest_block = adapter.latest_block().await?;
    let runtime = adapter.runtime_info().await?;
    let capabilities = adapter.probe_capabilities().await?;
    let account_balance = adapter.account_balance(account).await?;
    tracing::debug!(
        block_number = latest_block.number,
        spec_name = %runtime.spec_name,
        spec_version = runtime.spec_version,
        "Portaldot runtime snapshot collected"
    );
    tracing::info!(
        contracts_ready = capabilities.ink_ready(),
        "Portaldot capability probe completed"
    );
    Ok(CapabilityReport {
        network,
        latest_block,
        runtime,
        capabilities,
        account,
        account_balance,
    })
}
