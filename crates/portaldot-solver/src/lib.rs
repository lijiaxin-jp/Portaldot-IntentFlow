//! Risk-aware, deterministic route selection for a single Portaldot network.

use defi_domain::{Balance, BlockRef, GasWeight, NetworkId, Route, SwapIntent};
use defi_engine::{
    RiskConfig, RiskError, RiskQuote, RouteGraph, RouteQuote, enumerate_routes,
    quote_route_exact_in, score_route,
};
use portaldot_dex::PoolAdapter;
use serde::{Deserialize, Serialize};
use serde_json::to_vec;
use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SolverConfig {
    pub max_hops: usize,
    pub risk: RiskConfig,
    /// Chain execution cost already converted to the intent's output asset.
    pub gas_cost_output_units: Balance,
    pub failure_risk_bps: u32,
    pub min_net_output: Balance,
    pub gas_estimate: Option<GasWeight>,
}

impl Default for SolverConfig {
    fn default() -> Self {
        Self {
            max_hops: 2,
            risk: RiskConfig::default(),
            gas_cost_output_units: 0,
            failure_risk_bps: 0,
            min_net_output: 0,
            gas_estimate: None,
        }
    }
}

impl From<&portaldot_config::SolverConfig> for SolverConfig {
    fn from(config: &portaldot_config::SolverConfig) -> Self {
        Self {
            max_hops: config.max_hops,
            risk: RiskConfig {
                max_price_impact_bps: config.max_price_impact_bps,
                max_stale_blocks: config.max_stale_blocks,
                max_failure_risk_bps: config.max_failure_risk_bps,
                ..RiskConfig::default()
            },
            min_net_output: config.min_net_output,
            ..Self::default()
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SolverDecision {
    pub intent: SwapIntent,
    pub route: Route,
    pub commitment_hash: defi_domain::Hash,
    pub report_hash: defi_domain::Hash,
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
    pub gas_estimate: Option<GasWeight>,
    pub price_impact_cost: Balance,
    pub stale_state_penalty: Balance,
    pub failure_risk_penalty: Balance,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SolverReport {
    pub schema_version: u16,
    pub network: NetworkId,
    pub block: BlockRef,
    pub intent: SwapIntent,
    pub intent_hash: defi_domain::Hash,
    pub policy: SolverPolicyView,
    pub candidates: Vec<CandidateReport>,
    pub selected_route_hash: Option<defi_domain::Hash>,
    pub commitment_hash: Option<defi_domain::Hash>,
    pub accepted: bool,
    pub rejection_reason: Option<String>,
    pub report_hash: defi_domain::Hash,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SolverPolicyView {
    pub max_hops: usize,
    pub min_net_output: Balance,
    pub max_price_impact_bps: u32,
    pub max_stale_blocks: u64,
    pub max_failure_risk_bps: u32,
    pub gas_cost_output_units: Balance,
    pub gas_estimate: Option<GasWeight>,
    pub failure_risk_bps: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CandidateReport {
    pub route: Route,
    pub quote: Option<RouteQuoteView>,
    pub risk: Option<RiskQuoteView>,
    pub accepted: bool,
    pub rejection_reason: Option<String>,
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
    #[error("solver report could not be serialized: {0}")]
    ReportSerialization(String),
    #[error("minimum net output exceeds the signed score range")]
    InvalidConfiguration,
}

/// Selects the highest risk-adjusted route using one immutable graph snapshot.
pub fn solve(
    graph: &RouteGraph,
    intent: SwapIntent,
    config: SolverConfig,
) -> Result<SolverDecision, SolverError> {
    let report = evaluate(graph, intent.clone(), config)?;
    let Some(route_hash) = report.selected_route_hash else {
        return if report.candidates.is_empty() {
            Err(SolverError::NoRoute)
        } else {
            Err(SolverError::NoAcceptableRoute)
        };
    };
    let candidate = report
        .candidates
        .iter()
        .find(|candidate| candidate.route.route_hash == route_hash)
        .expect("selected route must be present in the report");
    let quote = candidate
        .quote
        .clone()
        .expect("accepted candidate must have a quote");
    let risk = candidate
        .risk
        .clone()
        .expect("accepted candidate must have a risk breakdown");
    Ok(SolverDecision {
        intent: report.intent,
        route: candidate.route.clone(),
        commitment_hash: report
            .commitment_hash
            .expect("accepted report must have a commitment hash"),
        report_hash: report.report_hash,
        quote,
        risk,
        accepted: true,
        rejection_reason: None,
    })
}

/// Evaluates every route and returns a stable, replayable explanation even when all routes fail.
pub fn evaluate(
    graph: &RouteGraph,
    intent: SwapIntent,
    config: SolverConfig,
) -> Result<SolverReport, SolverError> {
    intent.validate()?;
    if i128::try_from(config.min_net_output).is_err() {
        return Err(SolverError::InvalidConfiguration);
    }
    let routes = enumerate_routes(graph, intent.asset_in, intent.asset_out, config.max_hops)?;
    let min_net_output =
        i128::try_from(config.min_net_output).map_err(|_| SolverError::InvalidConfiguration)?;
    let policy = SolverPolicyView {
        max_hops: config.max_hops,
        min_net_output: config.min_net_output,
        max_price_impact_bps: config.risk.max_price_impact_bps,
        max_stale_blocks: config.risk.max_stale_blocks,
        max_failure_risk_bps: config.risk.max_failure_risk_bps,
        gas_cost_output_units: config.gas_cost_output_units,
        gas_estimate: config.gas_estimate,
        failure_risk_bps: config.failure_risk_bps,
    };
    let mut candidates = Vec::with_capacity(routes.len());
    let mut selected: Option<(defi_domain::Hash, i128)> = None;
    for route in routes {
        let candidate = match quote_route_exact_in(graph, &route, intent.amount_in) {
            Err(error) => rejected_candidate(route, None, None, error.to_string()),
            Ok(quote) => evaluate_quote(&intent, &route, quote, config, min_net_output),
        };
        if candidate.accepted {
            let score = candidate
                .risk
                .as_ref()
                .expect("accepted route has a score")
                .score;
            if selected.is_none_or(|(hash, best_score)| {
                score > best_score || (score == best_score && candidate.route.route_hash < hash)
            }) {
                selected = Some((candidate.route.route_hash, score));
            }
        }
        candidates.push(candidate);
    }
    let selected_route_hash = selected.map(|(hash, _)| hash);
    let commitment_hash = selected_route_hash.map(|_| {
        defi_engine::hash_intent_route(
            &intent,
            &candidates
                .iter()
                .find(|candidate| Some(candidate.route.route_hash) == selected_route_hash)
                .expect("selected route exists")
                .route,
        )
    });
    let accepted = selected_route_hash.is_some();
    let rejection_reason = if accepted {
        None
    } else if candidates.is_empty() {
        Some("no_candidate_routes".to_owned())
    } else {
        Some("all_candidate_routes_rejected".to_owned())
    };
    let intent_hash = defi_engine::hash_intent(&intent);
    let report_hash = report_hash(
        &graph.network,
        graph.block,
        &intent,
        intent_hash,
        &policy,
        &candidates,
        selected_route_hash,
        commitment_hash,
        accepted,
        rejection_reason.as_deref(),
    )?;
    Ok(SolverReport {
        schema_version: 1,
        network: graph.network.clone(),
        block: graph.block,
        intent,
        intent_hash,
        policy,
        candidates,
        selected_route_hash,
        commitment_hash,
        accepted,
        rejection_reason,
        report_hash,
    })
}

fn evaluate_quote(
    intent: &SwapIntent,
    route: &Route,
    quote: RouteQuote,
    config: SolverConfig,
    min_net_output: i128,
) -> CandidateReport {
    let quote_view = RouteQuoteView {
        amount_in: quote.amount_in,
        amount_out: quote.amount_out,
        fee_cost: quote.fee_cost,
        route_hash: quote.route.route_hash,
    };
    if quote.amount_out < intent.amount_out_min {
        return rejected_candidate(
            route.clone(),
            Some(quote_view),
            None,
            "amount_out_below_minimum".into(),
        );
    }
    if quote.fee_cost > intent.max_fee {
        return rejected_candidate(
            route.clone(),
            Some(quote_view),
            None,
            "fee_above_intent_maximum".into(),
        );
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
        Err(error) => {
            return rejected_candidate(
                route.clone(),
                Some(quote_view),
                None,
                risk_rejection_reason(error),
            );
        }
    };
    let risk_view = risk_view(risk, config.gas_estimate);
    if risk_view.score < min_net_output {
        return rejected_candidate(
            route.clone(),
            Some(quote_view),
            Some(risk_view),
            "net_output_below_minimum".into(),
        );
    }
    CandidateReport {
        route: route.clone(),
        quote: Some(quote_view),
        risk: Some(risk_view),
        accepted: true,
        rejection_reason: None,
    }
}

fn rejected_candidate(
    route: Route,
    quote: Option<RouteQuoteView>,
    risk: Option<RiskQuoteView>,
    reason: String,
) -> CandidateReport {
    CandidateReport {
        route,
        quote,
        risk,
        accepted: false,
        rejection_reason: Some(reason),
    }
}

fn risk_view(risk: RiskQuote, gas_estimate: Option<GasWeight>) -> RiskQuoteView {
    RiskQuoteView {
        expected_output: risk.expected_output,
        score: risk.score,
        price_impact_bps: risk.price_impact_bps,
        stale_blocks: risk.stale_blocks,
        failure_risk_bps: risk.failure_risk_bps,
        fee_cost: risk.fee_cost,
        gas_cost_output_units: risk.gas_cost_output_units,
        gas_estimate,
        price_impact_cost: risk.price_impact_cost,
        stale_state_penalty: risk.stale_state_penalty,
        failure_risk_penalty: risk.failure_risk_penalty,
    }
}

fn risk_rejection_reason(error: RiskError) -> String {
    match error {
        RiskError::PriceImpactTooHigh => "price_impact_above_limit",
        RiskError::StaleState => "pool_state_stale",
        RiskError::FailureRiskTooHigh => "failure_risk_above_limit",
        RiskError::ArithmeticOverflow => "risk_score_overflow",
    }
    .into()
}

#[allow(clippy::too_many_arguments)]
fn report_hash(
    network: &NetworkId,
    block: BlockRef,
    intent: &SwapIntent,
    intent_hash: defi_domain::Hash,
    policy: &SolverPolicyView,
    candidates: &[CandidateReport],
    selected_route_hash: Option<defi_domain::Hash>,
    commitment_hash: Option<defi_domain::Hash>,
    accepted: bool,
    rejection_reason: Option<&str>,
) -> Result<defi_domain::Hash, SolverError> {
    #[derive(Serialize)]
    struct ReportBody<'a> {
        schema_version: u16,
        network: &'a NetworkId,
        block: BlockRef,
        intent: &'a SwapIntent,
        intent_hash: defi_domain::Hash,
        policy: &'a SolverPolicyView,
        candidates: &'a [CandidateReport],
        selected_route_hash: Option<defi_domain::Hash>,
        commitment_hash: Option<defi_domain::Hash>,
        accepted: bool,
        rejection_reason: Option<&'a str>,
    }
    let body = ReportBody {
        schema_version: 1,
        network,
        block,
        intent,
        intent_hash,
        policy,
        candidates,
        selected_route_hash,
        commitment_hash,
        accepted,
        rejection_reason,
    };
    let bytes =
        to_vec(&body).map_err(|error| SolverError::ReportSerialization(error.to_string()))?;
    Ok(defi_engine::hash_bytes(&bytes))
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
