//! Evidence records used to make every decision reproducible.

mod paper;

use async_trait::async_trait;
use defi_domain::{EvidenceStatus, ExecutionEvidence, Hash, NetworkId};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};
use thiserror::Error;

pub use paper::{
    PaperAccount, PaperAccountConfig, PaperCandidate, PaperDecision, PaperDecisionResult,
    PaperEquity, PaperError,
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DeploymentEvidence {
    pub network: NetworkId,
    pub contract_kind: String,
    pub address: defi_domain::ContractId,
    pub code_hash: Hash,
    pub metadata_hash: Hash,
    pub deploy_tx_hash: Hash,
    pub deploy_block_hash: Hash,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QuoteEvidence {
    pub intent_id: Hash,
    pub route_hash: Hash,
    pub commitment_hash: Hash,
    pub amount_in: u128,
    pub amount_out: u128,
    pub fee: u128,
    pub gas: u128,
    pub score: i128,
    pub risk_fields: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StoredEvidence {
    pub execution: ExecutionEvidence,
    pub deployment: Option<DeploymentEvidence>,
    pub quote: Option<QuoteEvidence>,
    pub input_hash: Hash,
    pub route_hash: Hash,
    pub simulation_hash: Option<Hash>,
    pub receipt_hash: Option<Hash>,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum StoreError {
    #[error("evidence already exists for intent")]
    Duplicate,
    #[error("evidence was not found")]
    NotFound,
    #[error("store lock is poisoned")]
    Lock,
}

#[async_trait]
pub trait EvidenceStore: Send + Sync {
    async fn put(&self, evidence: StoredEvidence) -> Result<(), StoreError>;
    async fn get(&self, intent_id: Hash) -> Result<Option<StoredEvidence>, StoreError>;
    async fn update_status(
        &self,
        intent_id: Hash,
        status: EvidenceStatus,
    ) -> Result<(), StoreError>;
}

#[derive(Clone, Default)]
pub struct InMemoryEvidenceStore {
    records: Arc<RwLock<Vec<StoredEvidence>>>,
}

#[async_trait]
impl EvidenceStore for InMemoryEvidenceStore {
    async fn put(&self, evidence: StoredEvidence) -> Result<(), StoreError> {
        let mut records = self.records.write().map_err(|_| StoreError::Lock)?;
        if records
            .iter()
            .any(|record| record.execution.intent_id == evidence.execution.intent_id)
        {
            return Err(StoreError::Duplicate);
        }
        records.push(evidence);
        Ok(())
    }

    async fn get(&self, intent_id: Hash) -> Result<Option<StoredEvidence>, StoreError> {
        let records = self.records.read().map_err(|_| StoreError::Lock)?;
        Ok(records
            .iter()
            .find(|record| record.execution.intent_id == intent_id)
            .cloned())
    }

    async fn update_status(
        &self,
        intent_id: Hash,
        status: EvidenceStatus,
    ) -> Result<(), StoreError> {
        let mut records = self.records.write().map_err(|_| StoreError::Lock)?;
        let record = records
            .iter_mut()
            .find(|record| record.execution.intent_id == intent_id)
            .ok_or(StoreError::NotFound)?;
        record.execution.status = status;
        Ok(())
    }
}
