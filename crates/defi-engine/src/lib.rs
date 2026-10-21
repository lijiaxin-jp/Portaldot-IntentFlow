//! Deterministic routing and risk calculations.
//!
//! This crate contains the reusable pure-computation part of the former
//! research platform. It has no RPC, database, runtime, or chain-specific
//! dependency; a Portaldot adapter supplies snapshots and consumes the results.

mod fingerprint;
mod optimizer;
mod quote;
mod risk;
mod route;

pub use fingerprint::{hash_bytes, hash_intent, hash_intent_route, hash_route, replay_fingerprint};
pub use optimizer::{
    AmountInterval, OptimizedRoute, OptimizerConfig, OptimizerError, optimize_route_input,
};
pub use quote::{HopQuote, QuoteError, quote_constant_product, quote_pool_exact_in};
pub use risk::{RiskConfig, RiskError, RiskInput, RiskQuote, score_route};
pub use route::{RouteError, RouteGraph, RouteQuote, enumerate_routes, quote_route_exact_in};
