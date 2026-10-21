use defi_domain::{Balance, Route};
use thiserror::Error;

use crate::{RouteError, RouteGraph, RouteQuote, quote_route_exact_in};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AmountInterval {
    pub min: Balance,
    pub max: Balance,
}

impl AmountInterval {
    pub fn validate(self) -> Result<(), OptimizerError> {
        if self.min == 0 || self.min > self.max {
            return Err(OptimizerError::InvalidInterval);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OptimizerConfig {
    pub coarse_steps: u32,
    pub refinement_steps: u32,
}

impl Default for OptimizerConfig {
    fn default() -> Self {
        Self {
            coarse_steps: 16,
            refinement_steps: 8,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OptimizedRoute {
    pub quote: RouteQuote,
    pub evaluated_inputs: u32,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum OptimizerError {
    #[error("amount interval is invalid")]
    InvalidInterval,
    #[error("optimizer configuration has no search steps")]
    InvalidConfig,
    #[error("route quote failed for every candidate")]
    NoQuote,
    #[error("route quote failed: {0}")]
    Route(#[from] RouteError),
}

pub fn optimize_route_input(
    graph: &RouteGraph,
    route: &Route,
    interval: AmountInterval,
    config: OptimizerConfig,
) -> Result<OptimizedRoute, OptimizerError> {
    interval.validate()?;
    if config.coarse_steps == 0 || config.refinement_steps == 0 {
        return Err(OptimizerError::InvalidConfig);
    }
    let mut best: Option<RouteQuote> = None;
    let mut evaluated = 0;
    let coarse = config.coarse_steps.max(1);
    for index in 0..=coarse {
        let amount = interpolate(interval, index, coarse);
        if let Ok(quote) = quote_route_exact_in(graph, route, amount) {
            evaluated += 1;
            if best
                .as_ref()
                .is_none_or(|current| quote.amount_out > current.amount_out)
            {
                best = Some(quote);
            }
        }
    }
    let Some(mut best_quote) = best else {
        return Err(OptimizerError::NoQuote);
    };
    let span = interval.max.saturating_sub(interval.min) / coarse.max(1) as u128;
    let local_min = best_quote.amount_in.saturating_sub(span);
    let local_max = best_quote.amount_in.saturating_add(span).min(interval.max);
    for index in 0..=config.refinement_steps {
        let amount = interpolate(
            AmountInterval {
                min: local_min.max(interval.min),
                max: local_max,
            },
            index,
            config.refinement_steps,
        );
        if let Ok(quote) = quote_route_exact_in(graph, route, amount) {
            evaluated += 1;
            if quote.amount_out > best_quote.amount_out {
                best_quote = quote;
            }
        }
    }
    Ok(OptimizedRoute {
        quote: best_quote,
        evaluated_inputs: evaluated,
    })
}

fn interpolate(interval: AmountInterval, index: u32, steps: u32) -> Balance {
    if steps == 0 || interval.min == interval.max {
        return interval.min;
    }
    let width = interval.max - interval.min;
    interval.min + width.saturating_mul(index as u128) / steps as u128
}
