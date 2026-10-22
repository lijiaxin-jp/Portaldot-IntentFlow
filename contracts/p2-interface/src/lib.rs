#![cfg_attr(not(feature = "std"), no_std)]

use ink::prelude::{string::String, vec::Vec};
use ink::scale::{Decode, Encode};
#[cfg(feature = "std")]
use scale_info::TypeInfo;
use sha2::{Digest, Sha256};

pub type AccountId = <ink::env::DefaultEnvironment as ink::env::Environment>::AccountId;
pub type Balance = <ink::env::DefaultEnvironment as ink::env::Environment>::Balance;
pub type Hash = <ink::env::DefaultEnvironment as ink::env::Environment>::Hash;

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, PartialEq)]
#[cfg_attr(feature = "std", derive(TypeInfo))]
pub enum ContractError {
    Unauthorized,
    ZeroAddress,
    InvalidAmount,
    InsufficientBalance,
    InsufficientAllowance,
    ArithmeticOverflow,
    InvalidAssetPair,
    InvalidFee,
    InvalidDeadline,
    InvalidRoute,
    PoolNotAllowed,
    Paused,
    SlippageExceeded,
    FeeLimitExceeded,
    TokenCallFailed,
    PoolCallFailed,
    InsufficientLiquidity,
    InsufficientShares,
    UnbalancedLiquidity,
    StaleRoute,
    ReentrantCall,
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, PartialEq)]
#[cfg_attr(feature = "std", derive(TypeInfo))]
pub enum AssetId {
    Native,
    Contract(AccountId),
}

#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq)]
#[cfg_attr(feature = "std", derive(TypeInfo))]
pub struct SwapIntent {
    pub intent_id: Hash,
    pub owner: AccountId,
    pub asset_in: AssetId,
    pub asset_out: AssetId,
    pub amount_in: Balance,
    pub amount_out_min: Balance,
    pub max_fee: Balance,
    pub deadline: u64,
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, PartialEq)]
#[cfg_attr(feature = "std", derive(TypeInfo))]
pub struct RouteStep {
    pub pool_id: AccountId,
    pub asset_in: AssetId,
    pub asset_out: AssetId,
}

#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq)]
#[cfg_attr(feature = "std", derive(TypeInfo))]
pub struct ExecutionResult {
    pub amount_in: Balance,
    pub amount_out: Balance,
    pub fee: Balance,
    pub route_hash: Hash,
    pub solver: AccountId,
}

#[ink::trait_definition]
pub trait Psp22 {
    #[ink(message)]
    fn name(&self) -> String;

    #[ink(message)]
    fn symbol(&self) -> String;

    #[ink(message)]
    fn decimals(&self) -> u8;

    #[ink(message)]
    fn test_only(&self) -> bool;

    #[ink(message)]
    fn total_supply(&self) -> Balance;

    #[ink(message)]
    fn balance_of(&self, owner: AccountId) -> Balance;

    #[ink(message)]
    fn allowance(&self, owner: AccountId, spender: AccountId) -> Balance;

    #[ink(message)]
    fn transfer(&mut self, to: AccountId, amount: Balance) -> Result<(), ContractError>;

    #[ink(message)]
    fn approve(&mut self, spender: AccountId, amount: Balance) -> Result<(), ContractError>;

    #[ink(message)]
    fn transfer_from(
        &mut self,
        from: AccountId,
        to: AccountId,
        amount: Balance,
    ) -> Result<(), ContractError>;
}

#[ink::trait_definition]
pub trait AmmPool {
    #[ink(message)]
    fn admin(&self) -> AccountId;

    #[ink(message)]
    fn token_0(&self) -> AccountId;

    #[ink(message)]
    fn token_1(&self) -> AccountId;

    #[ink(message)]
    fn fee_bps(&self) -> u32;

    #[ink(message)]
    fn reserves(&self) -> (Balance, Balance);

    #[ink(message)]
    fn total_liquidity(&self) -> Balance;

    #[ink(message)]
    fn shares_of(&self, owner: AccountId) -> Balance;

    #[ink(message)]
    fn add_liquidity(
        &mut self,
        amount_0: Balance,
        amount_1: Balance,
        min_shares: Balance,
        deadline: u64,
    ) -> Result<Balance, ContractError>;

    #[ink(message)]
    fn remove_liquidity(
        &mut self,
        shares: Balance,
        amount_0_min: Balance,
        amount_1_min: Balance,
        deadline: u64,
    ) -> Result<(Balance, Balance), ContractError>;

    #[ink(message)]
    fn quote_exact_input(
        &self,
        asset_in: AccountId,
        amount_in: Balance,
    ) -> Result<Balance, ContractError>;

    #[ink(message)]
    fn quote_without_fee(
        &self,
        asset_in: AccountId,
        amount_in: Balance,
    ) -> Result<Balance, ContractError>;

    #[ink(message)]
    fn swap_exact_input(
        &mut self,
        asset_in: AccountId,
        amount_in: Balance,
        amount_out_min: Balance,
        deadline: u64,
        recipient: AccountId,
    ) -> Result<Balance, ContractError>;
}

#[ink::trait_definition]
pub trait IntentRouter {
    #[ink(message)]
    fn admin(&self) -> AccountId;

    #[ink(message)]
    fn set_pool_allowed(&mut self, pool: AccountId, allowed: bool) -> Result<(), ContractError>;

    #[ink(message)]
    fn is_pool_allowed(&self, pool: AccountId) -> bool;

    #[ink(message)]
    fn set_solver_authorized(
        &mut self,
        solver: AccountId,
        authorized: bool,
    ) -> Result<(), ContractError>;

    #[ink(message)]
    fn is_solver_authorized(&self, solver: AccountId) -> bool;

    #[ink(message)]
    fn set_paused(&mut self, paused: bool) -> Result<(), ContractError>;

    #[ink(message)]
    fn is_paused(&self) -> bool;

    #[ink(message)]
    fn execute(
        &mut self,
        intent: SwapIntent,
        steps: Vec<RouteStep>,
        snapshot_block_number: u64,
        snapshot_block_hash: Hash,
        expected_commitment: Hash,
    ) -> Result<ExecutionResult, ContractError>;
}

#[must_use]
pub fn hash_route(network: &str, block_number: u64, block_hash: Hash, steps: &[RouteStep]) -> Hash {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"intentflow-route-v1\0");
    bytes.extend_from_slice(&(network.len() as u32).to_le_bytes());
    bytes.extend_from_slice(network.as_bytes());
    bytes.extend_from_slice(&block_number.to_le_bytes());
    bytes.extend_from_slice(&block_hash.encode());
    bytes.extend_from_slice(&(steps.len() as u32).to_le_bytes());
    for step in steps {
        bytes.extend_from_slice(&step.pool_id.encode());
        put_asset(&mut bytes, step.asset_in);
        put_asset(&mut bytes, step.asset_out);
    }
    Hash::from(<[u8; 32]>::from(Sha256::digest(bytes)))
}

#[must_use]
pub fn hash_intent(intent: &SwapIntent) -> Hash {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"intentflow-intent-v1\0");
    bytes.extend_from_slice(&intent.intent_id.encode());
    bytes.extend_from_slice(&intent.owner.encode());
    put_asset(&mut bytes, intent.asset_in);
    put_asset(&mut bytes, intent.asset_out);
    bytes.extend_from_slice(&intent.amount_in.to_le_bytes());
    bytes.extend_from_slice(&intent.amount_out_min.to_le_bytes());
    bytes.extend_from_slice(&intent.max_fee.to_le_bytes());
    bytes.extend_from_slice(&intent.deadline.to_le_bytes());
    Hash::from(<[u8; 32]>::from(Sha256::digest(bytes)))
}

#[must_use]
pub fn hash_execution_commitment(intent: &SwapIntent, route_hash: Hash) -> Hash {
    let mut bytes = Vec::with_capacity(96);
    bytes.extend_from_slice(b"intentflow-execution-commitment-v1\0");
    bytes.extend_from_slice(&hash_intent(intent).encode());
    bytes.extend_from_slice(&route_hash.encode());
    Hash::from(<[u8; 32]>::from(Sha256::digest(bytes)))
}

fn put_asset(bytes: &mut Vec<u8>, asset: AssetId) {
    match asset {
        AssetId::Native => bytes.push(0),
        AssetId::Contract(account) => {
            bytes.push(1);
            bytes.extend_from_slice(&account.encode());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_hash_matches_the_chain_neutral_golden_vector() {
        let steps = [RouteStep {
            pool_id: AccountId::from([3; 32]),
            asset_in: AssetId::Contract(AccountId::from([1; 32])),
            asset_out: AssetId::Contract(AccountId::from([2; 32])),
        }];
        let expected = Hash::from([
            0x24, 0x16, 0x4a, 0x95, 0x1f, 0x80, 0x96, 0x00, 0xa4, 0xeb, 0x18, 0x84, 0x5b, 0x15,
            0x28, 0xe2, 0xa5, 0x6c, 0xc5, 0x99, 0x41, 0x5c, 0x22, 0x06, 0x85, 0xfa, 0x3b, 0x50,
            0x8d, 0x8d, 0xe4, 0x0b,
        ]);
        assert_eq!(
            hash_route("portaldot-testnet", 10, Hash::from([8; 32]), &steps),
            expected
        );
    }
}

pub fn account_is_zero(account: AccountId) -> bool {
    account == AccountId::from([0; 32])
}

pub fn asset_account(asset: AssetId) -> Option<AccountId> {
    match asset {
        AssetId::Native => None,
        AssetId::Contract(account) => Some(account),
    }
}
