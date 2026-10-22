//! Evidence records used to make every decision reproducible.

mod paper;

use async_trait::async_trait;
use defi_domain::{
    BlockRef, ContractId, EvidenceStatus, ExecutionEvidence, GasWeight, Hash, NetworkId,
    PoolSnapshot, Route, SwapIntent,
};
use serde::{Deserialize, Serialize};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use std::{
    path::Path,
    sync::{Arc, RwLock},
    time::{SystemTime, UNIX_EPOCH},
};
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
    pub intent: SwapIntent,
    pub route: Route,
    pub deployment: Option<DeploymentEvidence>,
    pub quote: Option<QuoteEvidence>,
    pub input_hash: Hash,
    pub route_hash: Hash,
    pub simulation_hash: Option<Hash>,
    pub receipt_hash: Option<Hash>,
    pub simulation: Option<SimulationEvidence>,
    pub receipt: Option<ReceiptEvidence>,
    pub events: Vec<StoredEvent>,
    pub failure_reason: Option<String>,
    pub created_at_ms: u64,
    pub simulation_duration_ms: Option<u64>,
    pub submission_duration_ms: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SimulationEvidence {
    pub call_hash: Hash,
    pub block: BlockRef,
    pub success: bool,
    pub gas_required: GasWeight,
    pub output: Vec<u8>,
    pub events: Vec<Vec<u8>>,
    pub failure_reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReceiptEvidence {
    pub tx_hash: Hash,
    pub block: BlockRef,
    pub extrinsic_index: u32,
    pub success: bool,
    pub gas_used: GasWeight,
    pub failure_reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StoredEvent {
    pub pallet: String,
    pub event: String,
    pub block: BlockRef,
    pub extrinsic_index: u32,
    pub event_index: u32,
    pub contract: Option<ContractId>,
    pub topics: Vec<Hash>,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunEvidence {
    pub run_id: String,
    pub network: NetworkId,
    pub runtime_version: String,
    pub start_block: u64,
    pub end_block: Option<u64>,
    pub start_block_hash: Option<Hash>,
    pub end_block_hash: Option<Hash>,
    pub code_revision: Option<String>,
    pub machine_profile: Option<String>,
    pub created_at_ms: u64,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum StoreError {
    #[error("evidence already exists for intent")]
    Duplicate,
    #[error("evidence was not found")]
    NotFound,
    #[error("store lock is poisoned")]
    Lock,
    #[error("evidence database error: {0}")]
    Database(String),
    #[error("evidence serialization error: {0}")]
    Serialization(String),
}

#[async_trait]
pub trait EvidenceStore: Send + Sync {
    async fn put(&self, evidence: StoredEvidence) -> Result<(), StoreError>;
    async fn update(&self, evidence: StoredEvidence) -> Result<(), StoreError>;
    async fn get(&self, intent_id: Hash) -> Result<Option<StoredEvidence>, StoreError>;
    async fn list(&self, limit: u32) -> Result<Vec<StoredEvidence>, StoreError>;
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

    async fn update(&self, evidence: StoredEvidence) -> Result<(), StoreError> {
        let mut records = self.records.write().map_err(|_| StoreError::Lock)?;
        let record = records
            .iter_mut()
            .find(|record| record.execution.intent_id == evidence.execution.intent_id)
            .ok_or(StoreError::NotFound)?;
        *record = evidence;
        Ok(())
    }

    async fn get(&self, intent_id: Hash) -> Result<Option<StoredEvidence>, StoreError> {
        let records = self.records.read().map_err(|_| StoreError::Lock)?;
        Ok(records
            .iter()
            .find(|record| record.execution.intent_id == intent_id)
            .cloned())
    }

    async fn list(&self, limit: u32) -> Result<Vec<StoredEvidence>, StoreError> {
        let records = self.records.read().map_err(|_| StoreError::Lock)?;
        Ok(records.iter().rev().take(limit as usize).cloned().collect())
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

#[derive(Clone)]
pub struct SqliteEvidenceStore {
    pool: SqlitePool,
}

impl SqliteEvidenceStore {
    pub async fn connect(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        if let Some(parent) = path
            .as_ref()
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)
                .map_err(|error| StoreError::Database(error.to_string()))?;
        }
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await
            .map_err(database_error)?;
        for statement in [
            "CREATE TABLE IF NOT EXISTS portaldot_runs (run_id TEXT PRIMARY KEY, network TEXT NOT NULL, runtime_version TEXT, start_block INTEGER, end_block INTEGER, code_revision TEXT, machine_profile TEXT, payload_json TEXT NOT NULL, created_at_ms INTEGER NOT NULL)",
            "CREATE TABLE IF NOT EXISTS portaldot_deployments (network TEXT NOT NULL, contract_kind TEXT NOT NULL, address TEXT NOT NULL, code_hash TEXT NOT NULL, metadata_hash TEXT NOT NULL, deploy_tx_hash TEXT NOT NULL, deploy_block_hash TEXT NOT NULL, payload_json TEXT NOT NULL, PRIMARY KEY (network, contract_kind))",
            "CREATE TABLE IF NOT EXISTS portaldot_pools (network TEXT NOT NULL, pool_address TEXT NOT NULL, token0 TEXT NOT NULL, token1 TEXT NOT NULL, fee_bps INTEGER NOT NULL, code_hash TEXT, observed_block INTEGER NOT NULL, payload_json TEXT NOT NULL, PRIMARY KEY (network, pool_address))",
            "CREATE TABLE IF NOT EXISTS portaldot_intents (intent_id TEXT PRIMARY KEY, owner TEXT NOT NULL, route_hash TEXT NOT NULL, amount_in TEXT NOT NULL, amount_out_min TEXT NOT NULL, deadline INTEGER NOT NULL, payload_json TEXT NOT NULL)",
            "CREATE TABLE IF NOT EXISTS portaldot_quotes (intent_id TEXT PRIMARY KEY, amount_out TEXT NOT NULL, fee TEXT NOT NULL, gas TEXT NOT NULL, score TEXT NOT NULL, risk_fields_json TEXT NOT NULL, payload_json TEXT NOT NULL)",
            "CREATE TABLE IF NOT EXISTS portaldot_executions (intent_id TEXT PRIMARY KEY, tx_hash TEXT, block_hash TEXT, status TEXT NOT NULL, amount_out TEXT, fee TEXT, event_hash TEXT, payload_json TEXT NOT NULL)",
            "CREATE TABLE IF NOT EXISTS portaldot_evidence (intent_id TEXT PRIMARY KEY, input_hash TEXT NOT NULL, route_hash TEXT NOT NULL, simulation_hash TEXT, receipt_hash TEXT, payload_json TEXT NOT NULL, created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL)",
            "CREATE INDEX IF NOT EXISTS portaldot_executions_status_idx ON portaldot_executions(status)",
        ] {
            sqlx::query(statement)
                .execute(&pool)
                .await
                .map_err(database_error)?;
        }
        Ok(Self { pool })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub async fn record_run(&self, run: &RunEvidence) -> Result<(), StoreError> {
        let payload = serde_json::to_string(run)
            .map_err(|error| StoreError::Serialization(error.to_string()))?;
        sqlx::query("INSERT INTO portaldot_runs (run_id, network, runtime_version, start_block, end_block, code_revision, machine_profile, payload_json, created_at_ms) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(run_id) DO UPDATE SET end_block = excluded.end_block, payload_json = excluded.payload_json")
            .bind(&run.run_id)
            .bind(&run.network.0)
            .bind(&run.runtime_version)
            .bind(i64::try_from(run.start_block).unwrap_or(i64::MAX))
            .bind(run.end_block.map(|value| i64::try_from(value).unwrap_or(i64::MAX)))
            .bind(&run.code_revision)
            .bind(&run.machine_profile)
            .bind(payload)
            .bind(i64::try_from(run.created_at_ms).unwrap_or(i64::MAX))
            .execute(&self.pool)
            .await
            .map_err(database_error)?;
        Ok(())
    }

    pub async fn record_pool(
        &self,
        network: &NetworkId,
        snapshot: &PoolSnapshot,
    ) -> Result<(), StoreError> {
        snapshot
            .validate()
            .map_err(|error| StoreError::Database(error.to_string()))?;
        let payload = serde_json::json!({"network": network, "snapshot": snapshot});
        let token_0 = serde_json::to_string(&snapshot.asset_0)
            .map_err(|error| StoreError::Serialization(error.to_string()))?;
        let token_1 = serde_json::to_string(&snapshot.asset_1)
            .map_err(|error| StoreError::Serialization(error.to_string()))?;
        sqlx::query("INSERT INTO portaldot_pools (network, pool_address, token0, token1, fee_bps, code_hash, observed_block, payload_json) VALUES (?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(network, pool_address) DO UPDATE SET token0 = excluded.token0, token1 = excluded.token1, fee_bps = excluded.fee_bps, code_hash = excluded.code_hash, observed_block = excluded.observed_block, payload_json = excluded.payload_json")
            .bind(&network.0)
            .bind(hex::encode(snapshot.pool_id.0))
            .bind(token_0)
            .bind(token_1)
            .bind(i64::from(snapshot.fee_bps))
            .bind(snapshot.code_hash.map(hex_hash))
            .bind(i64::try_from(snapshot.observed_at.number).unwrap_or(i64::MAX))
            .bind(payload.to_string())
            .execute(&self.pool)
            .await
            .map_err(database_error)?;
        Ok(())
    }
}

#[async_trait]
impl EvidenceStore for SqliteEvidenceStore {
    async fn put(&self, evidence: StoredEvidence) -> Result<(), StoreError> {
        let intent_id = hex_hash(evidence.execution.intent_id);
        let payload = serde_json::to_string(&evidence)
            .map_err(|error| StoreError::Serialization(error.to_string()))?;
        let now = now_ms();
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        let insert = sqlx::query("INSERT INTO portaldot_evidence (intent_id, input_hash, route_hash, simulation_hash, receipt_hash, payload_json, created_at_ms, updated_at_ms) VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(&intent_id)
            .bind(hex_hash(evidence.input_hash))
            .bind(hex_hash(evidence.route_hash))
            .bind(evidence.simulation_hash.map(hex_hash))
            .bind(evidence.receipt_hash.map(hex_hash))
            .bind(&payload)
            .bind(i64::try_from(now).unwrap_or(i64::MAX))
            .bind(i64::try_from(now).unwrap_or(i64::MAX))
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        let _ = insert;
        write_normalized_records(&mut tx, &evidence, &intent_id).await?;
        tx.commit().await.map_err(database_error)
    }

    async fn update(&self, evidence: StoredEvidence) -> Result<(), StoreError> {
        let intent_id = hex_hash(evidence.execution.intent_id);
        let payload = serde_json::to_string(&evidence)
            .map_err(|error| StoreError::Serialization(error.to_string()))?;
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        let result = sqlx::query("UPDATE portaldot_evidence SET input_hash = ?, route_hash = ?, simulation_hash = ?, receipt_hash = ?, payload_json = ?, updated_at_ms = ? WHERE intent_id = ?")
            .bind(hex_hash(evidence.input_hash))
            .bind(hex_hash(evidence.route_hash))
            .bind(evidence.simulation_hash.map(hex_hash))
            .bind(evidence.receipt_hash.map(hex_hash))
            .bind(&payload)
            .bind(i64::try_from(now_ms()).unwrap_or(i64::MAX))
            .bind(&intent_id)
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        if result.rows_affected() == 0 {
            return Err(StoreError::NotFound);
        }
        write_normalized_records(&mut tx, &evidence, &intent_id).await?;
        tx.commit().await.map_err(database_error)
    }

    async fn get(&self, intent_id: Hash) -> Result<Option<StoredEvidence>, StoreError> {
        let payload: Option<String> =
            sqlx::query_scalar("SELECT payload_json FROM portaldot_evidence WHERE intent_id = ?")
                .bind(hex_hash(intent_id))
                .fetch_optional(&self.pool)
                .await
                .map_err(database_error)?;
        payload
            .map(|payload| {
                serde_json::from_str(&payload)
                    .map_err(|error| StoreError::Serialization(error.to_string()))
            })
            .transpose()
    }

    async fn list(&self, limit: u32) -> Result<Vec<StoredEvidence>, StoreError> {
        let payloads: Vec<String> = sqlx::query_scalar(
            "SELECT payload_json FROM portaldot_evidence ORDER BY created_at_ms DESC LIMIT ?",
        )
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(database_error)?;
        payloads
            .into_iter()
            .map(|payload| {
                serde_json::from_str(&payload)
                    .map_err(|error| StoreError::Serialization(error.to_string()))
            })
            .collect()
    }

    async fn update_status(
        &self,
        intent_id: Hash,
        status: EvidenceStatus,
    ) -> Result<(), StoreError> {
        let mut evidence = self.get(intent_id).await?.ok_or(StoreError::NotFound)?;
        evidence.execution.status = status;
        self.update(evidence).await
    }
}

async fn write_normalized_records(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    evidence: &StoredEvidence,
    intent_id: &str,
) -> Result<(), StoreError> {
    let execution_payload = serde_json::to_string(evidence)
        .map_err(|error| StoreError::Serialization(error.to_string()))?;
    let execution = &evidence.execution;
    if let Some(deployment) = &evidence.deployment {
        let deployment_payload = serde_json::to_string(deployment)
            .map_err(|error| StoreError::Serialization(error.to_string()))?;
        sqlx::query("INSERT INTO portaldot_deployments (network, contract_kind, address, code_hash, metadata_hash, deploy_tx_hash, deploy_block_hash, payload_json) VALUES (?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(network, contract_kind) DO UPDATE SET address = excluded.address, code_hash = excluded.code_hash, metadata_hash = excluded.metadata_hash, deploy_tx_hash = excluded.deploy_tx_hash, deploy_block_hash = excluded.deploy_block_hash, payload_json = excluded.payload_json")
            .bind(&deployment.network.0)
            .bind(&deployment.contract_kind)
            .bind(hex::encode(deployment.address.0))
            .bind(hex_hash(deployment.code_hash))
            .bind(hex_hash(deployment.metadata_hash))
            .bind(hex_hash(deployment.deploy_tx_hash))
            .bind(hex_hash(deployment.deploy_block_hash))
            .bind(deployment_payload)
            .execute(&mut **tx)
            .await
            .map_err(database_error)?;
    }
    let intent_payload = serde_json::to_string(&evidence.intent)
        .map_err(|error| StoreError::Serialization(error.to_string()))?;
    sqlx::query("INSERT INTO portaldot_intents (intent_id, owner, route_hash, amount_in, amount_out_min, deadline, payload_json) VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT(intent_id) DO UPDATE SET owner = excluded.owner, route_hash = excluded.route_hash, amount_in = excluded.amount_in, amount_out_min = excluded.amount_out_min, deadline = excluded.deadline, payload_json = excluded.payload_json")
        .bind(intent_id)
        .bind(hex::encode(evidence.intent.owner.0))
        .bind(hex_hash(evidence.route_hash))
        .bind(evidence.intent.amount_in.to_string())
        .bind(evidence.intent.amount_out_min.to_string())
        .bind(i64::try_from(evidence.intent.deadline).unwrap_or(i64::MAX))
        .bind(intent_payload)
        .execute(&mut **tx)
        .await
        .map_err(database_error)?;
    sqlx::query("INSERT INTO portaldot_executions (intent_id, tx_hash, block_hash, status, amount_out, fee, event_hash, payload_json) VALUES (?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(intent_id) DO UPDATE SET tx_hash = excluded.tx_hash, block_hash = excluded.block_hash, status = excluded.status, amount_out = excluded.amount_out, fee = excluded.fee, event_hash = excluded.event_hash, payload_json = excluded.payload_json")
        .bind(intent_id)
        .bind(execution.tx_hash.map(hex_hash))
        .bind(execution.block_hash.map(hex_hash))
        .bind(format!("{:?}", execution.status))
        .bind(evidence.quote.as_ref().map(|quote| quote.amount_out.to_string()))
        .bind(evidence.quote.as_ref().map(|quote| quote.fee.to_string()))
        .bind(execution.event_hash.map(hex_hash))
        .bind(&execution_payload)
        .execute(&mut **tx)
        .await
        .map_err(database_error)?;
    if let Some(quote) = &evidence.quote {
        let quote_payload = serde_json::to_string(quote)
            .map_err(|error| StoreError::Serialization(error.to_string()))?;
        sqlx::query("INSERT INTO portaldot_quotes (intent_id, amount_out, fee, gas, score, risk_fields_json, payload_json) VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT(intent_id) DO UPDATE SET amount_out = excluded.amount_out, fee = excluded.fee, gas = excluded.gas, score = excluded.score, risk_fields_json = excluded.risk_fields_json, payload_json = excluded.payload_json")
            .bind(intent_id)
            .bind(quote.amount_out.to_string())
            .bind(quote.fee.to_string())
            .bind(quote.gas.to_string())
            .bind(quote.score.to_string())
            .bind(serde_json::to_string(&quote.risk_fields).map_err(|error| StoreError::Serialization(error.to_string()))?)
            .bind(quote_payload)
            .execute(&mut **tx)
            .await
            .map_err(database_error)?;
    }
    Ok(())
}

fn database_error(error: sqlx::Error) -> StoreError {
    match error {
        sqlx::Error::Database(database) if database.is_unique_violation() => StoreError::Duplicate,
        error => StoreError::Database(error.to_string()),
    }
}

fn hex_hash(hash: Hash) -> String {
    hex::encode(hash)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(u128::from(u64::MAX)) as u64
        })
}
