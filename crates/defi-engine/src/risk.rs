use defi_domain::Balance;
use thiserror::Error;

use crate::RouteQuote;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RiskConfig {
    pub max_price_impact_bps: u32,
    pub max_stale_blocks: u64,
    pub max_failure_risk_bps: u32,
    pub price_impact_weight_bps: u32,
    pub stale_penalty_per_block: Balance,
    pub failure_penalty_bps: u32,
}

impl Default for RiskConfig {
    fn default() -> Self {
        Self {
            max_price_impact_bps: 100,
            max_stale_blocks: 2,
            max_failure_risk_bps: 500,
            price_impact_weight_bps: 10_000,
            stale_penalty_per_block: 1,
            failure_penalty_bps: 10_000,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RiskInput {
    /// Gas cost converted to the route's output asset base units.
    pub gas_cost_output_units: Balance,
    pub failure_risk_bps: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RiskQuote {
    pub route_hash: defi_domain::Hash,
    pub amount_out: Balance,
    pub expected_output: Balance,
    pub price_impact_bps: u32,
    pub stale_blocks: u64,
    pub failure_risk_bps: u32,
    pub score: i128,
    pub fee_cost: Balance,
    pub gas_cost_output_units: Balance,
    pub price_impact_cost: Balance,
    pub stale_state_penalty: Balance,
    pub failure_risk_penalty: Balance,
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum RiskError {
    #[error("price impact exceeds the configured limit")]
    PriceImpactTooHigh,
    #[error("pool state is too stale")]
    StaleState,
    #[error("failure risk exceeds the configured limit")]
    FailureRiskTooHigh,
    #[error("risk score arithmetic overflowed")]
    ArithmeticOverflow,
}

pub fn score_route(
    quote: &RouteQuote,
    input: RiskInput,
    config: RiskConfig,
) -> Result<RiskQuote, RiskError> {
    let price_impact_bps = quote.price_impact_bps;
    let stale_blocks = quote.stale_blocks;
    if price_impact_bps > config.max_price_impact_bps {
        return Err(RiskError::PriceImpactTooHigh);
    }
    if stale_blocks > config.max_stale_blocks {
        return Err(RiskError::StaleState);
    }
    if input.failure_risk_bps > config.max_failure_risk_bps {
        return Err(RiskError::FailureRiskTooHigh);
    }
    let price_impact_penalty = quote
        .price_impact_cost
        .checked_mul(Balance::from(config.price_impact_weight_bps))
        .and_then(|value| value.checked_div(10_000))
        .ok_or(RiskError::ArithmeticOverflow)?;
    let stale_state_penalty = Balance::from(stale_blocks)
        .checked_mul(config.stale_penalty_per_block)
        .ok_or(RiskError::ArithmeticOverflow)?;
    let failure_risk_penalty = quote
        .expected_output
        .checked_mul(Balance::from(input.failure_risk_bps))
        .and_then(|value| value.checked_mul(Balance::from(config.failure_penalty_bps)))
        .and_then(|value| value.checked_div(100_000_000))
        .ok_or(RiskError::ArithmeticOverflow)?;
    let deductions = quote
        .fee_cost
        .checked_add(input.gas_cost_output_units)
        .and_then(|value| value.checked_add(price_impact_penalty))
        .and_then(|value| value.checked_add(stale_state_penalty))
        .and_then(|value| value.checked_add(failure_risk_penalty))
        .ok_or(RiskError::ArithmeticOverflow)?;
    let score = i128::try_from(quote.expected_output)
        .map_err(|_| RiskError::ArithmeticOverflow)?
        .checked_sub(i128::try_from(deductions).map_err(|_| RiskError::ArithmeticOverflow)?)
        .ok_or(RiskError::ArithmeticOverflow)?;
    Ok(RiskQuote {
        route_hash: quote.route.route_hash,
        amount_out: quote.amount_out,
        expected_output: quote.expected_output,
        price_impact_bps,
        stale_blocks,
        failure_risk_bps: input.failure_risk_bps,
        score,
        fee_cost: quote.fee_cost,
        gas_cost_output_units: input.gas_cost_output_units,
        price_impact_cost: price_impact_penalty,
        stale_state_penalty,
        failure_risk_penalty,
    })
}
