//! Portaldot pool adapter contracts.
//!
//! No exchange-specific adapter is imported here. A concrete adapter must
//! decode the ink! pool metadata and events after P0 confirms the deployed
//! contract ABI.

use async_trait::async_trait;
use defi_domain::{AssetId, ContractId, Hash, PoolKind, PoolSnapshot};
use serde::{Deserialize, Serialize};
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
pub struct PoolEvent {
    pub pool_id: ContractId,
    pub block: defi_domain::BlockRef,
    pub event_kind: String,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum PoolAdapterError {
    #[error("pool metadata is invalid: {0}")]
    InvalidMetadata(String),
    #[error("pool read failed: {0}")]
    Read(String),
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
        amount_in: u128,
    ) -> Result<u128, PoolAdapterError>;
    fn decode_event(
        &self,
        pool: ContractId,
        block: defi_domain::BlockRef,
        raw: &[u8],
    ) -> Result<PoolEvent, PoolAdapterError>;
}
