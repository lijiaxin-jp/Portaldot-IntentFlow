//! Risk-aware, deterministic route selection for a single Portaldot network.

use defi_domain::{Balance, Route, SwapIntent};
use defi_engine::{
    RiskConfig, RiskError, RiskQuote, RouteGraph, RouteQuote, enumerate_routes,
    quote_route_exact_in, score_route,
};
use portaldot_dex::PoolAdapter;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SolverConfig {
    pub max_hops: usize,
    pub risk: RiskConfig,
    /// Chain execution cost already converted to the intent's output asset.
    pub gas_cost_output_units: Balance,
    pub failure_risk_bps: u32,
}

impl Default for SolverConfig {
    fn default() -> Self {
        Self {
            max_hops: 2,
            risk: RiskConfig::default(),
            gas_cost_output_units: 0,
            failure_risk_bps: 0,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SolverDecision {
    pub intent: SwapIntent,
    pub route: Route,
    pub commitment_hash: defi_domain::Hash,
    pub quote: RouteQuoteView,
    pub risk: RiskQuoteView,
    pub accepted: bool,
    pub rejection_reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RouteQuoteView {
    pub amount_in: Balance,
    pub amount_out: Balance,
    pub fee_cost: Balance,
    pub route_hash: defi_domain::Hash,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RiskQuoteView {
    pub expected_output: Balance,
    pub score: i128,
    pub price_impact_bps: u32,
    pub stale_blocks: u64,
    pub failure_risk_bps: u32,
    pub fee_cost: Balance,
    pub gas_cost_output_units: Balance,
    pub price_impact_cost: Balance,
    pub stale_state_penalty: Balance,
    pub failure_risk_penalty: Balance,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum SolverError {
    #[error("intent is invalid: {0}")]
    InvalidIntent(#[from] defi_domain::DomainError),
    #[error("route graph is invalid: {0}")]
    InvalidGraph(#[from] defi_engine::RouteError),
    #[error("no candidate route exists")]
    NoRoute,
    #[error("no candidate route satisfies the intent constraints")]
    NoAcceptableRoute,
    #[error("risk policy rejected every candidate: {0}")]
    Risk(#[from] RiskError),
}

/// Selects the highest risk-adjusted route using one immutable graph snapshot.
pub fn solve(
    graph: &RouteGraph,
    intent: SwapIntent,
    config: SolverConfig,
) -> Result<SolverDecision, SolverError> {
    intent.validate()?;
    let routes = enumerate_routes(graph, intent.asset_in, intent.asset_out, config.max_hops)?;
    if routes.is_empty() {
        return Err(SolverError::NoRoute);
    }
    let mut best: Option<(Route, RouteQuote, RiskQuote)> = None;
    for route in routes {
        let quote = match quote_route_exact_in(graph, &route, intent.amount_in) {
            Ok(quote) => quote,
            Err(_) => continue,
        };
        if quote.amount_out < intent.amount_out_min || quote.fee_cost > intent.max_fee {
            continue;
        }
        let risk = match score_route(
            &quote,
            defi_engine::RiskInput {
                gas_cost_output_units: config.gas_cost_output_units,
                failure_risk_bps: config.failure_risk_bps,
            },
            config.risk,
        ) {
            Ok(risk) => risk,
            Err(_) => continue,
        };
        if best
            .as_ref()
            .is_none_or(|(_, _, current)| risk.score > current.score)
        {
            best = Some((route, quote, risk));
        }
    }
    let Some((route, quote, risk)) = best else {
        return Err(SolverError::NoAcceptableRoute);
    };
    let commitment_hash = defi_engine::hash_intent_route(&intent, &route);
    Ok(SolverDecision {
        intent,
        route,
        commitment_hash,
        quote: RouteQuoteView {
            amount_in: quote.amount_in,
            amount_out: quote.amount_out,
            fee_cost: quote.fee_cost,
            route_hash: quote.route.route_hash,
        },
        risk: RiskQuoteView {
            expected_output: risk.expected_output,
            score: risk.score,
            price_impact_bps: risk.price_impact_bps,
            stale_blocks: risk.stale_blocks,
            failure_risk_bps: risk.failure_risk_bps,
            fee_cost: risk.fee_cost,
            gas_cost_output_units: risk.gas_cost_output_units,
            price_impact_cost: risk.price_impact_cost,
            stale_state_penalty: risk.stale_state_penalty,
            failure_risk_penalty: risk.failure_risk_penalty,
        },
        accepted: true,
        rejection_reason: None,
    })
}

/// Keeps the adapter dependency explicit for future live discovery without
/// allowing the solver to call arbitrary contracts.
pub async fn discover_snapshots<A: PoolAdapter>(
    adapter: &A,
    pools: &[defi_domain::ContractId],
) -> Result<Vec<defi_domain::PoolSnapshot>, portaldot_dex::PoolAdapterError> {
    let mut snapshots = Vec::with_capacity(pools.len());
    for pool in pools {
        snapshots.push(adapter.snapshot(*pool).await?);
    }
    Ok(snapshots)
}
