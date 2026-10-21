use defi_domain::{AssetId, Balance, BlockRef, NetworkId, PoolSnapshot, Route, RouteStep};
use std::collections::HashSet;
use thiserror::Error;

use crate::quote::mul_div_floor;
use crate::{HopQuote, QuoteError, hash_route, quote_constant_product, quote_pool_exact_in};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouteGraph {
    pub network: NetworkId,
    pub block: BlockRef,
    pub pools: Vec<PoolSnapshot>,
}

impl RouteGraph {
    pub fn validate(&self) -> Result<(), RouteError> {
        if self.pools.is_empty() {
            return Err(RouteError::EmptyGraph);
        }
        if self.network.0.trim().is_empty() || self.block.hash == [0; 32] {
            return Err(RouteError::InvalidGraphIdentity);
        }
        let mut ids = HashSet::with_capacity(self.pools.len());
        for pool in &self.pools {
            if !ids.insert(pool.pool_id) {
                return Err(RouteError::DuplicatePool(pool.pool_id));
            }
            if pool.observed_at.number > self.block.number {
                return Err(RouteError::FuturePoolSnapshot(pool.pool_id));
            }
            if pool.observed_at.number == self.block.number
                && pool.observed_at.hash != self.block.hash
            {
                return Err(RouteError::ConflictingPoolSnapshot(pool.pool_id));
            }
        }
        for pool in &self.pools {
            pool.validate().map_err(RouteError::InvalidPool)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouteQuote {
    pub route: Route,
    pub amount_in: Balance,
    pub amount_out: Balance,
    /// Ideal output at each pool's spot price, before fees and price impact.
    pub expected_output: Balance,
    /// Fee loss measured in the final output asset's base units.
    pub fee_cost: Balance,
    /// Constant-product curve loss measured in the final output asset's base units.
    pub price_impact_cost: Balance,
    pub price_impact_bps: u32,
    pub stale_blocks: u64,
    pub hops: Vec<HopQuote>,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum RouteError {
    #[error("route graph must contain at least one pool")]
    EmptyGraph,
    #[error("route graph identity is invalid")]
    InvalidGraphIdentity,
    #[error("pool is duplicated: {0:?}")]
    DuplicatePool(defi_domain::ContractId),
    #[error("pool snapshot is invalid: {0}")]
    InvalidPool(defi_domain::DomainError),
    #[error("pool snapshot is newer than its route graph: {0:?}")]
    FuturePoolSnapshot(defi_domain::ContractId),
    #[error("pool snapshot has a different hash at the route block: {0:?}")]
    ConflictingPoolSnapshot(defi_domain::ContractId),
    #[error("route is invalid")]
    InvalidRoute,
    #[error("route hash does not match its encoded steps")]
    RouteHashMismatch,
    #[error("route references a pool that is absent from the graph")]
    MissingPool,
    #[error("route reuses a pool")]
    ReusedPool,
    #[error("route steps are disconnected")]
    Disconnected,
    #[error("route quote failed: {0}")]
    Quote(#[from] QuoteError),
}

pub fn enumerate_routes(
    graph: &RouteGraph,
    asset_in: AssetId,
    asset_out: AssetId,
    max_hops: usize,
) -> Result<Vec<Route>, RouteError> {
    graph.validate()?;
    if asset_in == asset_out || max_hops == 0 {
        return Err(RouteError::InvalidRoute);
    }
    if max_hops > 2 {
        return Err(RouteError::InvalidRoute);
    }
    let mut routes = Vec::new();
    for first in &graph.pools {
        let Some(first_out) = first.other_asset(asset_in) else {
            continue;
        };
        if first_out == asset_out {
            routes.push(make_route(
                graph,
                vec![RouteStep {
                    pool_id: first.pool_id,
                    asset_in,
                    asset_out,
                }],
            ));
        }
        if max_hops < 2 {
            continue;
        }
        for second in &graph.pools {
            if first.pool_id == second.pool_id {
                continue;
            }
            if second.other_asset(first_out) != Some(asset_out) {
                continue;
            }
            routes.push(make_route(
                graph,
                vec![
                    RouteStep {
                        pool_id: first.pool_id,
                        asset_in,
                        asset_out: first_out,
                    },
                    RouteStep {
                        pool_id: second.pool_id,
                        asset_in: first_out,
                        asset_out,
                    },
                ],
            ));
        }
    }
    routes.sort_by_key(|route| route.route_hash);
    routes.dedup_by_key(|route| route.route_hash);
    Ok(routes)
}

fn make_route(graph: &RouteGraph, steps: Vec<RouteStep>) -> Route {
    let route_hash = hash_route(&graph.network, graph.block, &steps);
    Route {
        network: graph.network.clone(),
        start_block: graph.block,
        steps,
        route_hash,
    }
}

pub fn quote_route_exact_in(
    graph: &RouteGraph,
    route: &Route,
    amount_in: Balance,
) -> Result<RouteQuote, RouteError> {
    graph.validate()?;
    route.validate(2).map_err(|_| RouteError::InvalidRoute)?;
    if route.network != graph.network || route.start_block != graph.block {
        return Err(RouteError::InvalidRoute);
    }
    if route.route_hash != hash_route(&graph.network, graph.block, &route.steps) {
        return Err(RouteError::RouteHashMismatch);
    }
    let mut seen = Vec::with_capacity(route.steps.len());
    if amount_in == 0 {
        return Err(RouteError::Quote(QuoteError::ZeroInput));
    }
    let mut current = amount_in;
    let mut no_fee_current = amount_in;
    let mut spot_current = amount_in;
    let mut stale_blocks = 0;
    let mut hops = Vec::with_capacity(route.steps.len());
    for step in &route.steps {
        if seen.contains(&step.pool_id) {
            return Err(RouteError::ReusedPool);
        }
        seen.push(step.pool_id);
        let pool = graph
            .pools
            .iter()
            .find(|pool| pool.pool_id == step.pool_id)
            .ok_or(RouteError::MissingPool)?;
        if !pool.contains(step.asset_in) || pool.other_asset(step.asset_in) != Some(step.asset_out)
        {
            return Err(RouteError::Disconnected);
        }
        let quote = quote_pool_exact_in(pool, step.asset_in, current)?;
        let (reserve_in, reserve_out) = pool
            .reserves_for(step.asset_in)
            .ok_or(RouteError::Disconnected)?;
        no_fee_current =
            quote_constant_product(reserve_in, reserve_out, no_fee_current, 0)?.amount_out;
        spot_current = mul_div_floor(spot_current, reserve_out, reserve_in)?;
        stale_blocks = stale_blocks.max(graph.block.number.saturating_sub(pool.observed_at.number));
        current = quote.amount_out;
        hops.push(quote);
    }
    let fee_cost = no_fee_current.saturating_sub(current);
    let price_impact_cost = spot_current.saturating_sub(no_fee_current);
    let price_impact_bps = if spot_current == 0 {
        0
    } else {
        u32::try_from(mul_div_floor(price_impact_cost, 10_000, spot_current)?).unwrap_or(u32::MAX)
    };
    Ok(RouteQuote {
        route: route.clone(),
        amount_in,
        amount_out: current,
        expected_output: spot_current,
        fee_cost,
        price_impact_cost,
        price_impact_bps,
        stale_blocks,
        hops,
    })
}
