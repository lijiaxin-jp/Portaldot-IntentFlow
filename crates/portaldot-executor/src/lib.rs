//! Execution boundary for the ink! IntentRouter.
//!
//! Encoding and signing are kept behind this boundary.  The executor never
//! accepts arbitrary selectors or destinations; it only builds the fixed
//! IntentRouter entry point after a successful dry-run.

use defi_domain::{
    AccountId, AssetId, Balance, BlockRef, ContractId, EvidenceStatus, ExecutionEvidence, Hash,
    Route, SwapIntent,
};
use evidence_store::{
    EvidenceStore, QuoteEvidence, ReceiptEvidence, SimulationEvidence, StoreError, StoredEvent,
    StoredEvidence,
};
use parity_scale_codec::{Decode, Encode};
use portaldot_client::{
    ChainAdapter, ContractCall, SimulationResult, TransactionSigner, TransactionSubmitter,
    TxReceipt,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::Path,
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecutionPolicy {
    pub enabled: bool,
    pub dry_run_required: bool,
    pub max_fee: Balance,
    /// Selector decoded from the verified IntentRouter metadata at P0/P2.
    pub router_selector: [u8; 4],
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExecutionRequest {
    pub router: ContractId,
    pub intent: SwapIntent,
    pub route: Route,
    pub commitment_hash: Hash,
    pub solver: AccountId,
    #[serde(default)]
    pub quote: Option<QuoteEvidence>,
}

impl ExecutionRequest {
    pub fn from_solver_decision(
        router: ContractId,
        solver: AccountId,
        decision: &portaldot_solver::SolverDecision,
    ) -> Result<Self, ExecutionError> {
        if !decision.accepted {
            return Err(ExecutionError::InvalidRequest(
                decision
                    .rejection_reason
                    .clone()
                    .unwrap_or_else(|| "solver decision was rejected".into()),
            ));
        }
        let risk_fields = serde_json::to_value(&decision.risk)
            .map_err(|error| ExecutionError::InvalidRequest(error.to_string()))?;
        Ok(Self {
            router,
            intent: decision.intent.clone(),
            route: decision.route.clone(),
            commitment_hash: decision.commitment_hash,
            solver,
            quote: Some(QuoteEvidence {
                intent_id: decision.intent.intent_id,
                route_hash: decision.quote.route_hash,
                commitment_hash: decision.commitment_hash,
                amount_in: decision.quote.amount_in,
                amount_out: decision.quote.amount_out,
                fee: decision.quote.fee_cost,
                gas: decision
                    .risk
                    .gas_estimate
                    .map_or(0, |gas| u128::from(gas.ref_time)),
                score: decision.risk.score,
                risk_fields,
            }),
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PreparedSimulation {
    pub call_hash: Hash,
    pub block: BlockRef,
    pub result: SimulationResult,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ExecutionError {
    #[error("execution is disabled by configuration")]
    Disabled,
    #[error("dry-run is required before submission")]
    DryRunRequired,
    #[error("dry-run result does not match the call being submitted")]
    SimulationMismatch,
    #[error("dry-run failed: {0}")]
    DryRunFailed(String),
    #[error("request is invalid: {0}")]
    InvalidRequest(String),
    #[error("adapter error: {0}")]
    Adapter(#[from] portaldot_client::AdapterError),
    #[error("evidence store error: {0}")]
    Store(#[from] StoreError),
}

/// ABI encoder supplied from the verified IntentRouter metadata.
pub trait IntentRouterEncoder: Send + Sync {
    fn encode_arguments(
        &self,
        intent: &SwapIntent,
        route: &Route,
        commitment_hash: Hash,
    ) -> Result<Vec<u8>, ExecutionError>;

    fn message_selector(&self) -> Option<[u8; 4]> {
        None
    }

    fn validate_output(&self, _output: &[u8]) -> Result<(), String> {
        Ok(())
    }

    fn validate_receipt(&self, _receipt: &TxReceipt, _router: ContractId) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct InkIntentRouterEncoder {
    selector: [u8; 4],
    route_executed_topic: Hash,
}

#[derive(Deserialize)]
struct RouterBundle {
    spec: RouterSpec,
}

#[derive(Deserialize)]
struct RouterSpec {
    messages: Vec<RouterMessage>,
    events: Vec<RouterEvent>,
}

#[derive(Deserialize)]
struct RouterMessage {
    label: String,
    selector: String,
}

#[derive(Deserialize)]
struct RouterEvent {
    label: String,
    #[serde(rename = "signature_topic")]
    signature_topic: String,
}

impl InkIntentRouterEncoder {
    pub fn from_contract_bundle(path: impl AsRef<Path>) -> Result<Self, ExecutionError> {
        let source = fs::read_to_string(path)
            .map_err(|error| ExecutionError::InvalidRequest(error.to_string()))?;
        let bundle: RouterBundle = serde_json::from_str(&source)
            .map_err(|error| ExecutionError::InvalidRequest(error.to_string()))?;
        let selector = bundle
            .spec
            .messages
            .into_iter()
            .find(|message| message.label.rsplit("::").next() == Some("execute"))
            .ok_or_else(|| {
                ExecutionError::InvalidRequest("Router metadata has no execute message".into())
            })?
            .selector;
        let route_executed_topic = bundle
            .spec
            .events
            .into_iter()
            .find(|event| event.label.rsplit("::").next() == Some("RouteExecuted"))
            .ok_or_else(|| {
                ExecutionError::InvalidRequest("Router metadata has no RouteExecuted event".into())
            })?
            .signature_topic;
        Ok(Self {
            selector: decode_hex::<4>(&selector)?,
            route_executed_topic: decode_hex::<32>(&route_executed_topic)?,
        })
    }
}

impl IntentRouterEncoder for InkIntentRouterEncoder {
    fn encode_arguments(
        &self,
        intent: &SwapIntent,
        route: &Route,
        commitment_hash: Hash,
    ) -> Result<Vec<u8>, ExecutionError> {
        use intentflow_contract_interface as contract;

        let contract_intent = contract::SwapIntent {
            intent_id: intent.intent_id.into(),
            owner: contract::AccountId::from(intent.owner.0),
            asset_in: contract_asset(intent.asset_in),
            asset_out: contract_asset(intent.asset_out),
            amount_in: intent.amount_in,
            amount_out_min: intent.amount_out_min,
            max_fee: intent.max_fee,
            deadline: intent.deadline,
        };
        let steps: Vec<contract::RouteStep> = route
            .steps
            .iter()
            .map(|step| contract::RouteStep {
                pool_id: contract::AccountId::from(step.pool_id.0),
                asset_in: contract_asset(step.asset_in),
                asset_out: contract_asset(step.asset_out),
            })
            .collect();
        Ok((
            contract_intent,
            steps,
            route.start_block.number,
            route.start_block.hash,
            commitment_hash,
        )
            .encode())
    }

    fn message_selector(&self) -> Option<[u8; 4]> {
        Some(self.selector)
    }

    fn validate_output(&self, output: &[u8]) -> Result<(), String> {
        use intentflow_contract_interface::{ContractError, ExecutionResult};

        let mut remaining = output;
        let result = Result::<ExecutionResult, ContractError>::decode(&mut remaining)
            .map_err(|error| format!("Router output decode failed: {error}"))?;
        if !remaining.is_empty() {
            return Err("Router output contains trailing SCALE bytes".into());
        }
        result
            .map(|_| ())
            .map_err(|error| format!("Router rejected the route: {error:?}"))
    }

    fn validate_receipt(&self, receipt: &TxReceipt, router: ContractId) -> Result<(), String> {
        if !receipt.success {
            return Err(receipt
                .failure_reason
                .clone()
                .unwrap_or_else(|| "Router extrinsic failed".into()));
        }
        let emitted = receipt.event_records.iter().any(|event| {
            event.pallet == "Contracts"
                && event.event == "ContractEmitted"
                && event.contract == Some(router)
                && event.topics.contains(&self.route_executed_topic)
        });
        if !emitted {
            return Err("Router call did not emit RouteExecuted".into());
        }
        Ok(())
    }
}

fn contract_asset(asset: AssetId) -> intentflow_contract_interface::AssetId {
    use intentflow_contract_interface as contract;

    match asset {
        AssetId::Native => contract::AssetId::Native,
        AssetId::Contract(id) => contract::AssetId::Contract(contract::AccountId::from(id.0)),
    }
}

fn decode_hex<const N: usize>(value: &str) -> Result<[u8; N], ExecutionError> {
    let bytes = hex::decode(value.strip_prefix("0x").unwrap_or(value))
        .map_err(|error| ExecutionError::InvalidRequest(error.to_string()))?;
    bytes.try_into().map_err(|bytes: Vec<u8>| {
        ExecutionError::InvalidRequest(format!("metadata has {} bytes; expected {N}", bytes.len()))
    })
}

pub async fn dry_run<A: ChainAdapter, E: IntentRouterEncoder>(
    adapter: &A,
    encoder: &E,
    request: &ExecutionRequest,
    policy: ExecutionPolicy,
) -> Result<PreparedSimulation, ExecutionError> {
    validate_request(request, policy)?;
    let call = make_call(encoder, request, policy)?;
    tracing::debug!(
        hops = request.route.steps.len(),
        "running IntentRouter dry-run"
    );
    let block = request.route.start_block;
    let result = adapter.dry_run_at(call.clone(), block).await?;
    if !result.success {
        tracing::warn!("IntentRouter dry-run rejected the route");
        return Err(ExecutionError::DryRunFailed(
            result
                .failure_reason
                .clone()
                .unwrap_or_else(|| "unknown failure".to_string()),
        ));
    }
    encoder
        .validate_output(&result.output)
        .map_err(ExecutionError::DryRunFailed)?;
    tracing::info!(
        gas_ref_time = result.gas_required.ref_time,
        gas_proof_size = result.gas_required.proof_size,
        "IntentRouter dry-run passed"
    );
    Ok(PreparedSimulation {
        call_hash: fingerprint_call(&call, block),
        block,
        result,
    })
}

pub async fn submit<A: TransactionSubmitter, E: IntentRouterEncoder, S: TransactionSigner>(
    adapter: &A,
    encoder: &E,
    signer: &S,
    request: &ExecutionRequest,
    policy: ExecutionPolicy,
    simulation: Option<&PreparedSimulation>,
) -> Result<TxReceipt, ExecutionError> {
    if !policy.enabled {
        tracing::warn!("execution submission rejected because execution is disabled");
        return Err(ExecutionError::Disabled);
    }
    if !policy.dry_run_required {
        return Err(ExecutionError::InvalidRequest(
            "IntentRouter submission cannot disable the dry-run gate".into(),
        ));
    }
    validate_request(request, policy)?;
    if request.intent.max_fee > policy.max_fee {
        return Err(ExecutionError::InvalidRequest(
            "intent fee exceeds execution policy".to_string(),
        ));
    }
    if signer.account_id() != request.solver {
        return Err(ExecutionError::InvalidRequest(
            "signer account does not match the requested solver".to_string(),
        ));
    }
    let call = make_call(encoder, request, policy)?;
    let Some(simulation) = simulation else {
        return Err(ExecutionError::DryRunRequired);
    };
    if !simulation.result.success {
        return Err(ExecutionError::DryRunRequired);
    }
    let block = request.route.start_block;
    if simulation.block != block || simulation.call_hash != fingerprint_call(&call, block) {
        return Err(ExecutionError::SimulationMismatch);
    }
    tracing::info!(
        hops = request.route.steps.len(),
        "submitting preflighted IntentRouter call"
    );
    let mut receipt = adapter
        .submit(call, simulation.result.gas_required, signer)
        .await
        .map_err(ExecutionError::from)?;
    if let Err(reason) = encoder.validate_receipt(&receipt, request.router) {
        receipt.success = false;
        receipt.failure_reason = Some(reason);
    }
    tracing::info!(
        success = receipt.success,
        block_number = receipt.block.number,
        "IntentRouter call finalized"
    );
    Ok(receipt)
}

pub async fn execute_and_record<
    A: ChainAdapter + TransactionSubmitter,
    E: IntentRouterEncoder,
    S: TransactionSigner,
    St: EvidenceStore,
>(
    adapter: &A,
    encoder: &E,
    signer: &S,
    request: &ExecutionRequest,
    policy: ExecutionPolicy,
    store: &St,
) -> Result<TxReceipt, ExecutionError> {
    validate_request(request, policy)?;
    let input_hash = hash_serialized(request)?;
    let mut evidence = StoredEvidence {
        execution: ExecutionEvidence {
            network: request.route.network.clone(),
            intent_id: request.intent.intent_id,
            route_hash: request.route.route_hash,
            commitment_hash: request.commitment_hash,
            simulation_hash: None,
            tx_hash: None,
            block_hash: None,
            event_hash: None,
            metadata_hash: None,
            code_hash: None,
            status: EvidenceStatus::Planned,
        },
        intent: request.intent.clone(),
        route: request.route.clone(),
        deployment: None,
        quote: request.quote.clone(),
        input_hash,
        route_hash: request.route.route_hash,
        simulation_hash: None,
        receipt_hash: None,
        simulation: None,
        receipt: None,
        events: Vec::new(),
        failure_reason: None,
        created_at_ms: now_ms(),
        simulation_duration_ms: None,
        submission_duration_ms: None,
    };
    store.put(evidence.clone()).await?;

    let simulation_started = Instant::now();
    let simulation = match dry_run(adapter, encoder, request, policy).await {
        Ok(simulation) => simulation,
        Err(error) => {
            evidence.execution.status = EvidenceStatus::Rejected;
            evidence.failure_reason = Some(error.to_string());
            evidence.simulation_duration_ms = Some(elapsed_ms(simulation_started));
            store.update(evidence).await?;
            return Err(error);
        }
    };
    evidence.simulation_duration_ms = Some(elapsed_ms(simulation_started));
    evidence.simulation_hash = Some(hash_serialized(&simulation)?);
    evidence.execution.simulation_hash = evidence.simulation_hash;
    evidence.execution.status = EvidenceStatus::Simulated;
    evidence.simulation = Some(SimulationEvidence {
        call_hash: simulation.call_hash,
        block: simulation.block,
        success: simulation.result.success,
        gas_required: simulation.result.gas_required,
        output: simulation.result.output.clone(),
        events: simulation.result.events.clone(),
        failure_reason: simulation.result.failure_reason.clone(),
    });
    store.update(evidence.clone()).await?;

    let submission_started = Instant::now();
    let receipt = match submit(adapter, encoder, signer, request, policy, Some(&simulation)).await {
        Ok(receipt) => receipt,
        Err(error) => {
            evidence.failure_reason = Some(error.to_string());
            evidence.submission_duration_ms = Some(elapsed_ms(submission_started));
            store.update(evidence).await?;
            return Err(error);
        }
    };
    evidence.submission_duration_ms = Some(elapsed_ms(submission_started));
    evidence.execution.status = if receipt.success {
        EvidenceStatus::Confirmed
    } else {
        EvidenceStatus::Rejected
    };
    evidence.execution.tx_hash = Some(receipt.tx_hash);
    evidence.execution.block_hash = Some(receipt.block.hash);
    evidence.receipt = Some(ReceiptEvidence {
        tx_hash: receipt.tx_hash,
        block: receipt.block,
        extrinsic_index: receipt.extrinsic_index,
        success: receipt.success,
        gas_used: receipt.gas_used,
        failure_reason: receipt.failure_reason.clone(),
    });
    evidence.events = receipt
        .event_records
        .iter()
        .map(|event| StoredEvent {
            pallet: event.pallet.clone(),
            event: event.event.clone(),
            block: event.block,
            extrinsic_index: event.extrinsic_index,
            event_index: event.event_index,
            contract: event.contract,
            topics: event.topics.clone(),
            data: event.data.clone(),
        })
        .collect();
    if !evidence.events.is_empty() {
        evidence.execution.event_hash = Some(hash_serialized(&evidence.events)?);
    }
    evidence.failure_reason = receipt.failure_reason.clone();
    evidence.receipt_hash = Some(hash_serialized(&receipt)?);
    store.update(evidence).await?;
    Ok(receipt)
}

fn hash_serialized<T: Serialize>(value: &T) -> Result<Hash, ExecutionError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| ExecutionError::InvalidRequest(error.to_string()))?;
    Ok(Sha256::digest(bytes).into())
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(u128::from(u64::MAX)) as u64
        })
}

fn validate_request(
    request: &ExecutionRequest,
    policy: ExecutionPolicy,
) -> Result<(), ExecutionError> {
    request
        .intent
        .validate()
        .map_err(|error| ExecutionError::InvalidRequest(error.to_string()))?;
    if request.router.is_zero() || request.solver.is_zero() {
        return Err(ExecutionError::InvalidRequest(
            "router and solver must be non-zero".to_string(),
        ));
    }
    if policy.router_selector == [0; 4] {
        return Err(ExecutionError::InvalidRequest(
            "router selector must come from verified metadata".to_string(),
        ));
    }
    if request.route.route_hash == [0; 32] {
        return Err(ExecutionError::InvalidRequest(
            "route hash must be non-zero".to_string(),
        ));
    }
    request
        .route
        .validate(2)
        .map_err(|error| ExecutionError::InvalidRequest(error.to_string()))?;
    if request.route.network.0.trim().is_empty() || request.route.start_block.hash == [0; 32] {
        return Err(ExecutionError::InvalidRequest(
            "route network and block identity must be set".to_string(),
        ));
    }
    let first = request.route.steps.first().ok_or_else(|| {
        ExecutionError::InvalidRequest("route must contain at least one step".to_string())
    })?;
    let last = request.route.steps.last().ok_or_else(|| {
        ExecutionError::InvalidRequest("route must contain at least one step".to_string())
    })?;
    if first.asset_in != request.intent.asset_in || last.asset_out != request.intent.asset_out {
        return Err(ExecutionError::InvalidRequest(
            "route endpoints do not match the intent".to_string(),
        ));
    }
    if request.route.route_hash
        != defi_engine::hash_route(
            &request.route.network,
            request.route.start_block,
            &request.route.steps,
        )
    {
        return Err(ExecutionError::InvalidRequest(
            "route hash does not match the route steps".to_string(),
        ));
    }
    if request.commitment_hash != defi_engine::hash_intent_route(&request.intent, &request.route) {
        return Err(ExecutionError::InvalidRequest(
            "intent-route commitment does not match the request".to_string(),
        ));
    }
    if let Some(quote) = &request.quote {
        if quote.intent_id != request.intent.intent_id
            || quote.route_hash != request.route.route_hash
            || quote.commitment_hash != request.commitment_hash
            || quote.amount_in != request.intent.amount_in
        {
            return Err(ExecutionError::InvalidRequest(
                "quote evidence does not match the intent and route".into(),
            ));
        }
    }
    Ok(())
}

fn make_call<E: IntentRouterEncoder>(
    encoder: &E,
    request: &ExecutionRequest,
    policy: ExecutionPolicy,
) -> Result<ContractCall, ExecutionError> {
    if encoder
        .message_selector()
        .is_some_and(|selector| selector != policy.router_selector)
    {
        return Err(ExecutionError::InvalidRequest(
            "configured router selector does not match the verified bundle".into(),
        ));
    }
    Ok(ContractCall {
        origin: request.solver,
        contract: request.router,
        selector: policy.router_selector,
        input: encoder.encode_arguments(
            &request.intent,
            &request.route,
            request.commitment_hash,
        )?,
        attached_value: 0,
    })
}

fn fingerprint_call(call: &ContractCall, block: BlockRef) -> Hash {
    let mut hasher = Sha256::new();
    hasher.update(b"intentflow-router-call-v1\0");
    hasher.update(call.origin.0);
    hasher.update(call.contract.0);
    hasher.update(block.number.to_le_bytes());
    hasher.update(block.hash);
    hasher.update(call.selector);
    hasher.update((call.input.len() as u64).to_le_bytes());
    hasher.update(&call.input);
    hasher.update(call.attached_value.to_le_bytes());
    hasher.finalize().into()
}
