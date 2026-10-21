//! Execution boundary for the ink! IntentRouter.
//!
//! Encoding and signing are kept behind this boundary.  The executor never
//! accepts arbitrary selectors or destinations; it only builds the fixed
//! IntentRouter entry point after a successful dry-run.

use async_trait::async_trait;
use defi_domain::{AccountId, Balance, ContractId, Hash, Route, SwapIntent};
use portaldot_client::{ChainAdapter, ContractCall, SignedCall, SimulationResult, TxReceipt};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
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
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PreparedSimulation {
    pub call_hash: Hash,
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
}

#[async_trait]
pub trait Signer: Send + Sync {
    async fn sign(
        &self,
        call: &ContractCall,
        account: AccountId,
    ) -> Result<Vec<u8>, ExecutionError>;
}

/// ABI encoder supplied from the verified IntentRouter metadata.
pub trait IntentRouterEncoder: Send + Sync {
    fn encode_arguments(
        &self,
        intent: &SwapIntent,
        route: &Route,
        commitment_hash: Hash,
    ) -> Result<Vec<u8>, ExecutionError>;
}

pub async fn dry_run<A: ChainAdapter, E: IntentRouterEncoder>(
    adapter: &A,
    encoder: &E,
    request: &ExecutionRequest,
    policy: ExecutionPolicy,
) -> Result<PreparedSimulation, ExecutionError> {
    validate_request(request, policy)?;
    let call = make_call(encoder, request, policy)?;
    let result = adapter.dry_run(call.clone()).await?;
    if !result.success {
        return Err(ExecutionError::DryRunFailed(
            result
                .failure_reason
                .clone()
                .unwrap_or_else(|| "unknown failure".to_string()),
        ));
    }
    Ok(PreparedSimulation {
        call_hash: fingerprint_call(&call),
        result,
    })
}

pub async fn submit<A: ChainAdapter, E: IntentRouterEncoder, S: Signer>(
    adapter: &A,
    encoder: &E,
    signer: &S,
    request: &ExecutionRequest,
    policy: ExecutionPolicy,
    simulation: Option<&PreparedSimulation>,
) -> Result<TxReceipt, ExecutionError> {
    if !policy.enabled {
        return Err(ExecutionError::Disabled);
    }
    validate_request(request, policy)?;
    if request.intent.max_fee > policy.max_fee {
        return Err(ExecutionError::InvalidRequest(
            "intent fee exceeds execution policy".to_string(),
        ));
    }
    let call = make_call(encoder, request, policy)?;
    if policy.dry_run_required {
        let Some(simulation) = simulation else {
            return Err(ExecutionError::DryRunRequired);
        };
        if !simulation.result.success {
            return Err(ExecutionError::DryRunRequired);
        }
        if simulation.call_hash != fingerprint_call(&call) {
            return Err(ExecutionError::SimulationMismatch);
        }
    }
    let signed_payload = signer.sign(&call, request.solver).await?;
    adapter
        .submit(SignedCall {
            call,
            signer: request.solver,
            signed_payload,
        })
        .await
        .map_err(ExecutionError::from)
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
    Ok(())
}

fn make_call<E: IntentRouterEncoder>(
    encoder: &E,
    request: &ExecutionRequest,
    policy: ExecutionPolicy,
) -> Result<ContractCall, ExecutionError> {
    Ok(ContractCall {
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

fn fingerprint_call(call: &ContractCall) -> Hash {
    let mut hasher = Sha256::new();
    hasher.update(b"intentflow-router-call-v1\0");
    hasher.update(call.contract.0);
    hasher.update(call.selector);
    hasher.update((call.input.len() as u64).to_le_bytes());
    hasher.update(&call.input);
    hasher.update(call.attached_value.to_le_bytes());
    hasher.finalize().into()
}
