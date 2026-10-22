//! Chain-neutral value types shared by the Portaldot solver and adapters.
//!
//! The types in this crate deliberately do not know about a particular address
//! encoding, RPC method, exchange, or database. Portaldot account and
//! contract identifiers are represented as their canonical 32-byte form; the
//! client is responsible for converting the runtime representation.

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const FEE_DENOMINATOR_BPS: u32 = 10_000;

pub type Hash = [u8; 32];
pub type Balance = u128;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct GasWeight {
    pub ref_time: u64,
    pub proof_size: u64,
}

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
pub struct AccountId(pub [u8; 32]);

impl AccountId {
    #[must_use]
    pub fn is_zero(self) -> bool {
        self.0 == [0; 32]
    }
}

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
pub struct ContractId(pub [u8; 32]);

impl ContractId {
    #[must_use]
    pub const fn zero() -> Self {
        Self([0; 32])
    }

    #[must_use]
    pub fn is_zero(self) -> bool {
        self.0 == [0; 32]
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct NetworkId(pub String);

impl NetworkId {
    pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(DomainError::EmptyIdentity("network"));
        }
        Ok(Self(value))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub enum AssetId {
    Native,
    Contract(ContractId),
}

impl AssetId {
    #[must_use]
    pub fn is_zero_contract(self) -> bool {
        match self {
            Self::Native => false,
            Self::Contract(id) => id.is_zero(),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct BlockRef {
    pub number: u64,
    pub hash: Hash,
}

impl BlockRef {
    pub fn new(number: u64, hash: Hash) -> Result<Self, DomainError> {
        if hash == [0; 32] {
            return Err(DomainError::ZeroHash("block"));
        }
        Ok(Self { number, hash })
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum PoolKind {
    ConstantProduct,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PoolSnapshot {
    pub pool_id: ContractId,
    pub kind: PoolKind,
    pub asset_0: AssetId,
    pub asset_1: AssetId,
    pub reserve_0: Balance,
    pub reserve_1: Balance,
    pub fee_bps: u32,
    pub observed_at: BlockRef,
    pub code_hash: Option<Hash>,
}

impl PoolSnapshot {
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.pool_id.is_zero() {
            return Err(DomainError::ZeroIdentity("pool"));
        }
        if self.observed_at.hash == [0; 32] {
            return Err(DomainError::ZeroHash("pool observation block"));
        }
        if self.asset_0 == self.asset_1 || self.asset_0 > self.asset_1 {
            return Err(DomainError::InvalidAssetPair);
        }
        if self.asset_0.is_zero_contract() || self.asset_1.is_zero_contract() {
            return Err(DomainError::ZeroIdentity("asset"));
        }
        if self.reserve_0 == 0 || self.reserve_1 == 0 {
            return Err(DomainError::ZeroLiquidity);
        }
        if self.fee_bps > FEE_DENOMINATOR_BPS {
            return Err(DomainError::InvalidFee(self.fee_bps));
        }
        if self.code_hash == Some([0; 32]) {
            return Err(DomainError::ZeroHash("pool code"));
        }
        Ok(())
    }

    #[must_use]
    pub fn contains(&self, asset: AssetId) -> bool {
        self.asset_0 == asset || self.asset_1 == asset
    }

    #[must_use]
    pub fn other_asset(&self, asset: AssetId) -> Option<AssetId> {
        if self.asset_0 == asset {
            Some(self.asset_1)
        } else if self.asset_1 == asset {
            Some(self.asset_0)
        } else {
            None
        }
    }

    #[must_use]
    pub fn reserves_for(&self, asset_in: AssetId) -> Option<(Balance, Balance)> {
        if self.asset_0 == asset_in {
            Some((self.reserve_0, self.reserve_1))
        } else if self.asset_1 == asset_in {
            Some((self.reserve_1, self.reserve_0))
        } else {
            None
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SwapIntent {
    pub intent_id: Hash,
    pub owner: AccountId,
    pub asset_in: AssetId,
    pub asset_out: AssetId,
    pub amount_in: Balance,
    pub amount_out_min: Balance,
    /// Maximum fee loss allowed by the intent, in output-asset base units.
    pub max_fee: Balance,
    pub deadline: u64,
}

impl SwapIntent {
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.intent_id == [0; 32] {
            return Err(DomainError::ZeroHash("intent"));
        }
        if self.owner.is_zero() {
            return Err(DomainError::ZeroIdentity("owner"));
        }
        if self.asset_in == self.asset_out
            || self.asset_in.is_zero_contract()
            || self.asset_out.is_zero_contract()
        {
            return Err(DomainError::InvalidAssetPair);
        }
        if self.amount_in == 0 {
            return Err(DomainError::ZeroAmount("amount_in"));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct RouteStep {
    pub pool_id: ContractId,
    pub asset_in: AssetId,
    pub asset_out: AssetId,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Route {
    pub network: NetworkId,
    pub start_block: BlockRef,
    pub steps: Vec<RouteStep>,
    pub route_hash: Hash,
}

impl Route {
    pub fn validate(&self, max_hops: usize) -> Result<(), DomainError> {
        if self.steps.is_empty() || self.steps.len() > max_hops {
            return Err(DomainError::InvalidRouteLength);
        }
        if self.route_hash == [0; 32] {
            return Err(DomainError::ZeroHash("route"));
        }
        for (index, step) in self.steps.iter().enumerate() {
            if step.pool_id.is_zero() || step.asset_in == step.asset_out {
                return Err(DomainError::InvalidRouteStep);
            }
            if index > 0 && self.steps[index - 1].asset_out != step.asset_in {
                return Err(DomainError::DisconnectedRoute);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RiskBreakdown {
    pub expected_output: Balance,
    pub fee_cost: Balance,
    pub gas_cost: Balance,
    pub price_impact_cost: Balance,
    pub stale_state_penalty: Balance,
    pub failure_risk_penalty: Balance,
    pub score: i128,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExecutionEvidence {
    pub network: NetworkId,
    pub intent_id: Hash,
    pub route_hash: Hash,
    pub commitment_hash: Hash,
    pub simulation_hash: Option<Hash>,
    pub tx_hash: Option<Hash>,
    pub block_hash: Option<Hash>,
    pub event_hash: Option<Hash>,
    pub metadata_hash: Option<Hash>,
    pub code_hash: Option<Hash>,
    pub status: EvidenceStatus,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum EvidenceStatus {
    Planned,
    Simulated,
    Submitted,
    Confirmed,
    Rejected,
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum DomainError {
    #[error("{0} identity must not be zero")]
    ZeroIdentity(&'static str),
    #[error("{0} hash must not be zero")]
    ZeroHash(&'static str),
    #[error("{0} must not be empty")]
    EmptyIdentity(&'static str),
    #[error("assets must be distinct and canonically ordered")]
    InvalidAssetPair,
    #[error("pool reserves must be greater than zero")]
    ZeroLiquidity,
    #[error("fee {0} bps exceeds the denominator")]
    InvalidFee(u32),
    #[error("{0} must be greater than zero")]
    ZeroAmount(&'static str),
    #[error("route length is outside the configured bound")]
    InvalidRouteLength,
    #[error("route step is invalid")]
    InvalidRouteStep,
    #[error("route steps are disconnected")]
    DisconnectedRoute,
}
