#![cfg_attr(not(feature = "std"), no_std, no_main)]

#[ink::contract]
mod amm_pool {
    use ink::codegen::TraitCallBuilder;
    use ink::storage::Mapping;
    use intentflow_contract_interface::{account_is_zero, AmmPool, ContractError, Psp22};
    use ruint::aliases::U512;

    const FEE_DENOMINATOR: u128 = 10_000;

    #[ink(storage)]
    pub struct AmmPoolContract {
        token_0: AccountId,
        token_1: AccountId,
        fee_bps: u32,
        admin: AccountId,
        reserve_0: Balance,
        reserve_1: Balance,
        total_liquidity: Balance,
        shares: Mapping<AccountId, Balance>,
        entered: bool,
    }

    #[ink(event)]
    pub struct LiquidityAdded {
        #[ink(topic)]
        provider: AccountId,
        amount_0: Balance,
        amount_1: Balance,
        shares: Balance,
    }

    #[ink(event)]
    pub struct LiquidityRemoved {
        #[ink(topic)]
        provider: AccountId,
        amount_0: Balance,
        amount_1: Balance,
        shares: Balance,
    }

    #[ink(event)]
    pub struct Swap {
        #[ink(topic)]
        sender: AccountId,
        #[ink(topic)]
        recipient: AccountId,
        asset_in: AccountId,
        asset_out: AccountId,
        amount_in: Balance,
        amount_out: Balance,
        fee_paid: Balance,
    }

    impl AmmPoolContract {
        #[ink(constructor)]
        pub fn new(
            token_0: AccountId,
            token_1: AccountId,
            fee_bps: u32,
        ) -> Result<Self, ContractError> {
            if account_is_zero(token_0) || account_is_zero(token_1) || token_0 == token_1 {
                return Err(ContractError::InvalidAssetPair);
            }
            if token_0 > token_1 {
                return Err(ContractError::InvalidAssetPair);
            }
            if fee_bps >= FEE_DENOMINATOR as u32 {
                return Err(ContractError::InvalidFee);
            }
            let admin = Self::env().caller();
            if account_is_zero(admin) {
                return Err(ContractError::ZeroAddress);
            }
            Ok(Self {
                token_0,
                token_1,
                fee_bps,
                admin,
                reserve_0: 0,
                reserve_1: 0,
                total_liquidity: 0,
                shares: Mapping::default(),
                entered: false,
            })
        }

        fn revert(error: ContractError) -> ! {
            ink::env::return_value(
                ink::env::ReturnFlags::REVERT,
                &Result::<(), ContractError>::Err(error),
            )
        }

        fn transfer_from(
            token_account: AccountId,
            from: AccountId,
            to: AccountId,
            amount: Balance,
        ) {
            let mut token: ink::contract_ref!(Psp22) = token_account.into();
            if !matches!(
                token
                    .call_mut()
                    .transfer_from(from, to, amount)
                    .try_invoke(),
                Ok(Ok(Ok(())))
            ) {
                Self::revert(ContractError::TokenCallFailed);
            }
        }

        fn transfer(token_account: AccountId, to: AccountId, amount: Balance) {
            let mut token: ink::contract_ref!(Psp22) = token_account.into();
            if !matches!(
                token.call_mut().transfer(to, amount).try_invoke(),
                Ok(Ok(Ok(())))
            ) {
                Self::revert(ContractError::TokenCallFailed);
            }
        }

        fn quote(
            reserve_in: Balance,
            reserve_out: Balance,
            amount_in: Balance,
            fee_bps: u32,
        ) -> Result<Balance, ContractError> {
            if amount_in == 0 {
                return Err(ContractError::InvalidAmount);
            }
            if reserve_in == 0 || reserve_out == 0 {
                return Err(ContractError::InsufficientLiquidity);
            }
            if fee_bps >= FEE_DENOMINATOR as u32 {
                return Err(ContractError::InvalidFee);
            }
            let fee_factor = U512::from(
                FEE_DENOMINATOR
                    .checked_sub(u128::from(fee_bps))
                    .ok_or(ContractError::ArithmeticOverflow)?,
            );
            let scaled_input = U512::from(amount_in)
                .checked_mul(fee_factor)
                .ok_or(ContractError::ArithmeticOverflow)?;
            let numerator = scaled_input
                .checked_mul(U512::from(reserve_out))
                .ok_or(ContractError::ArithmeticOverflow)?;
            let denominator = U512::from(reserve_in)
                .checked_mul(U512::from(FEE_DENOMINATOR))
                .and_then(|value| value.checked_add(scaled_input))
                .ok_or(ContractError::ArithmeticOverflow)?;
            if denominator == U512::ZERO {
                return Err(ContractError::ArithmeticOverflow);
            }
            let amount_out = u128::try_from(
                numerator
                    .checked_div(denominator)
                    .ok_or(ContractError::ArithmeticOverflow)?,
            )
            .map_err(|_| ContractError::ArithmeticOverflow)?;
            if amount_out == 0 {
                return Err(ContractError::InvalidAmount);
            }
            if amount_out >= reserve_out {
                return Err(ContractError::InsufficientLiquidity);
            }
            Ok(amount_out)
        }

        fn mint_liquidity_shares(
            &self,
            amount_0: Balance,
            amount_1: Balance,
        ) -> Result<Balance, ContractError> {
            if self.total_liquidity == 0 {
                return integer_sqrt_product(amount_0, amount_1);
            }
            if self.reserve_0 == 0 || self.reserve_1 == 0 {
                return Err(ContractError::InsufficientLiquidity);
            }
            let left = U512::from(amount_0)
                .checked_mul(U512::from(self.reserve_1))
                .ok_or(ContractError::ArithmeticOverflow)?;
            let right = U512::from(amount_1)
                .checked_mul(U512::from(self.reserve_0))
                .ok_or(ContractError::ArithmeticOverflow)?;
            if left != right {
                return Err(ContractError::UnbalancedLiquidity);
            }
            let shares_0 = mul_div(amount_0, self.total_liquidity, self.reserve_0)?;
            let shares_1 = mul_div(amount_1, self.total_liquidity, self.reserve_1)?;
            Ok(shares_0.min(shares_1))
        }
    }

    impl AmmPool for AmmPoolContract {
        #[ink(message)]
        fn admin(&self) -> AccountId {
            self.admin
        }

        #[ink(message)]
        fn token_0(&self) -> AccountId {
            self.token_0
        }

        #[ink(message)]
        fn token_1(&self) -> AccountId {
            self.token_1
        }

        #[ink(message)]
        fn fee_bps(&self) -> u32 {
            self.fee_bps
        }

        #[ink(message)]
        fn reserves(&self) -> (Balance, Balance) {
            (self.reserve_0, self.reserve_1)
        }

        #[ink(message)]
        fn total_liquidity(&self) -> Balance {
            self.total_liquidity
        }

        #[ink(message)]
        fn shares_of(&self, owner: AccountId) -> Balance {
            self.shares.get(owner).unwrap_or_default()
        }

        #[ink(message)]
        fn add_liquidity(
            &mut self,
            amount_0: Balance,
            amount_1: Balance,
            min_shares: Balance,
            deadline: u64,
        ) -> Result<Balance, ContractError> {
            if self.env().block_number() as u64 > deadline {
                return Err(ContractError::InvalidDeadline);
            }
            if amount_0 == 0 || amount_1 == 0 {
                return Err(ContractError::InvalidAmount);
            }
            let minted = self.mint_liquidity_shares(amount_0, amount_1)?;
            if minted == 0 || minted < min_shares {
                return Err(ContractError::SlippageExceeded);
            }
            let provider = self.env().caller();
            let provider_shares = self
                .shares_of(provider)
                .checked_add(minted)
                .ok_or(ContractError::ArithmeticOverflow)?;
            let total_liquidity = self
                .total_liquidity
                .checked_add(minted)
                .ok_or(ContractError::ArithmeticOverflow)?;
            let reserve_0 = self
                .reserve_0
                .checked_add(amount_0)
                .ok_or(ContractError::ArithmeticOverflow)?;
            let reserve_1 = self
                .reserve_1
                .checked_add(amount_1)
                .ok_or(ContractError::ArithmeticOverflow)?;
            if self.entered {
                return Err(ContractError::ReentrantCall);
            }
            self.entered = true;
            Self::transfer_from(self.token_0, provider, self.env().account_id(), amount_0);
            Self::transfer_from(self.token_1, provider, self.env().account_id(), amount_1);
            self.reserve_0 = reserve_0;
            self.reserve_1 = reserve_1;
            self.total_liquidity = total_liquidity;
            self.shares.insert(provider, &provider_shares);
            self.entered = false;
            self.env().emit_event(LiquidityAdded {
                provider,
                amount_0,
                amount_1,
                shares: minted,
            });
            Ok(minted)
        }

        #[ink(message)]
        fn remove_liquidity(
            &mut self,
            shares: Balance,
            amount_0_min: Balance,
            amount_1_min: Balance,
            deadline: u64,
        ) -> Result<(Balance, Balance), ContractError> {
            if self.env().block_number() as u64 > deadline {
                return Err(ContractError::InvalidDeadline);
            }
            if shares == 0 || self.total_liquidity == 0 {
                return Err(ContractError::InvalidAmount);
            }
            let provider = self.env().caller();
            let owned_shares = self.shares_of(provider);
            if owned_shares < shares {
                return Err(ContractError::InsufficientShares);
            }
            let amount_0 = mul_div(self.reserve_0, shares, self.total_liquidity)?;
            let amount_1 = mul_div(self.reserve_1, shares, self.total_liquidity)?;
            if amount_0 < amount_0_min || amount_1 < amount_1_min {
                return Err(ContractError::SlippageExceeded);
            }
            let new_reserve_0 = self
                .reserve_0
                .checked_sub(amount_0)
                .ok_or(ContractError::ArithmeticOverflow)?;
            let new_reserve_1 = self
                .reserve_1
                .checked_sub(amount_1)
                .ok_or(ContractError::ArithmeticOverflow)?;
            if self.entered {
                return Err(ContractError::ReentrantCall);
            }
            self.entered = true;
            Self::transfer(self.token_0, provider, amount_0);
            Self::transfer(self.token_1, provider, amount_1);
            self.reserve_0 = new_reserve_0;
            self.reserve_1 = new_reserve_1;
            self.total_liquidity = self
                .total_liquidity
                .checked_sub(shares)
                .ok_or(ContractError::ArithmeticOverflow)?;
            let remaining_shares = owned_shares
                .checked_sub(shares)
                .ok_or(ContractError::ArithmeticOverflow)?;
            self.shares.insert(provider, &remaining_shares);
            self.entered = false;
            self.env().emit_event(LiquidityRemoved {
                provider,
                amount_0,
                amount_1,
                shares,
            });
            Ok((amount_0, amount_1))
        }

        #[ink(message)]
        fn quote_exact_input(
            &self,
            asset_in: AccountId,
            amount_in: Balance,
        ) -> Result<Balance, ContractError> {
            let (reserve_in, reserve_out) = if asset_in == self.token_0 {
                (self.reserve_0, self.reserve_1)
            } else if asset_in == self.token_1 {
                (self.reserve_1, self.reserve_0)
            } else {
                return Err(ContractError::InvalidAssetPair);
            };
            Self::quote(reserve_in, reserve_out, amount_in, self.fee_bps)
        }

        #[ink(message)]
        fn quote_without_fee(
            &self,
            asset_in: AccountId,
            amount_in: Balance,
        ) -> Result<Balance, ContractError> {
            let (reserve_in, reserve_out) = if asset_in == self.token_0 {
                (self.reserve_0, self.reserve_1)
            } else if asset_in == self.token_1 {
                (self.reserve_1, self.reserve_0)
            } else {
                return Err(ContractError::InvalidAssetPair);
            };
            Self::quote(reserve_in, reserve_out, amount_in, 0)
        }

        #[ink(message)]
        fn swap_exact_input(
            &mut self,
            asset_in: AccountId,
            amount_in: Balance,
            amount_out_min: Balance,
            deadline: u64,
            recipient: AccountId,
        ) -> Result<Balance, ContractError> {
            if self.env().block_number() as u64 > deadline {
                return Err(ContractError::InvalidDeadline);
            }
            if account_is_zero(recipient) || amount_in == 0 {
                return Err(ContractError::InvalidAmount);
            }
            let (token_out, reserve_in, reserve_out, input_is_0) = if asset_in == self.token_0 {
                (self.token_1, self.reserve_0, self.reserve_1, true)
            } else if asset_in == self.token_1 {
                (self.token_0, self.reserve_1, self.reserve_0, false)
            } else {
                return Err(ContractError::InvalidAssetPair);
            };
            let amount_out = Self::quote(reserve_in, reserve_out, amount_in, self.fee_bps)?;
            if amount_out < amount_out_min {
                return Err(ContractError::SlippageExceeded);
            }
            let new_reserve_in = reserve_in
                .checked_add(amount_in)
                .ok_or(ContractError::ArithmeticOverflow)?;
            let new_reserve_out = reserve_out
                .checked_sub(amount_out)
                .ok_or(ContractError::ArithmeticOverflow)?;
            let fee_paid = u128::try_from(
                U512::from(amount_in)
                    .checked_mul(U512::from(self.fee_bps))
                    .ok_or(ContractError::ArithmeticOverflow)?
                    .checked_div(U512::from(FEE_DENOMINATOR))
                    .ok_or(ContractError::ArithmeticOverflow)?,
            )
            .map_err(|_| ContractError::ArithmeticOverflow)?;
            let sender = self.env().caller();
            if self.entered {
                return Err(ContractError::ReentrantCall);
            }
            self.entered = true;
            Self::transfer_from(asset_in, sender, self.env().account_id(), amount_in);
            Self::transfer(token_out, recipient, amount_out);
            if input_is_0 {
                self.reserve_0 = new_reserve_in;
                self.reserve_1 = new_reserve_out;
            } else {
                self.reserve_1 = new_reserve_in;
                self.reserve_0 = new_reserve_out;
            }
            self.entered = false;
            self.env().emit_event(Swap {
                sender,
                recipient,
                asset_in,
                asset_out: token_out,
                amount_in,
                amount_out,
                fee_paid,
            });
            Ok(amount_out)
        }
    }

    fn mul_div(a: Balance, b: Balance, denominator: Balance) -> Result<Balance, ContractError> {
        if denominator == 0 {
            return Err(ContractError::InsufficientLiquidity);
        }
        let result = U512::from(a)
            .checked_mul(U512::from(b))
            .ok_or(ContractError::ArithmeticOverflow)?
            .checked_div(U512::from(denominator))
            .ok_or(ContractError::ArithmeticOverflow)?;
        u128::try_from(result).map_err(|_| ContractError::ArithmeticOverflow)
    }

    fn integer_sqrt_product(a: Balance, b: Balance) -> Result<Balance, ContractError> {
        let product = U512::from(a)
            .checked_mul(U512::from(b))
            .ok_or(ContractError::ArithmeticOverflow)?;
        let mut low = U512::ZERO;
        let mut high = U512::from(u128::MAX);
        while low < high {
            let distance = high
                .checked_sub(low)
                .ok_or(ContractError::ArithmeticOverflow)?;
            let half_distance = distance
                .checked_div(U512::from(2))
                .ok_or(ContractError::ArithmeticOverflow)?;
            let odd_adjustment = distance
                .checked_rem(U512::from(2))
                .ok_or(ContractError::ArithmeticOverflow)?;
            let mid = low
                .checked_add(half_distance)
                .and_then(|value| value.checked_add(odd_adjustment))
                .ok_or(ContractError::ArithmeticOverflow)?;
            let square = mid
                .checked_mul(mid)
                .ok_or(ContractError::ArithmeticOverflow)?;
            if square <= product {
                low = mid;
            } else {
                high = mid
                    .checked_sub(U512::from(1))
                    .ok_or(ContractError::ArithmeticOverflow)?;
            }
        }
        u128::try_from(low).map_err(|_| ContractError::ArithmeticOverflow)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[ink::test]
        fn exact_input_quote_matches_solver_integer_formula() {
            assert_eq!(AmmPoolContract::quote(1_000, 2_000, 100, 30), Ok(181));
            assert_eq!(AmmPoolContract::quote(1_000, 2_000, 100, 0), Ok(181));
        }

        #[ink::test]
        fn quote_rejects_zero_and_empty_reserves() {
            assert_eq!(
                AmmPoolContract::quote(1, 1, 0, 30),
                Err(ContractError::InvalidAmount)
            );
            assert_eq!(
                AmmPoolContract::quote(0, 1, 1, 30),
                Err(ContractError::InsufficientLiquidity)
            );
        }

        #[ink::test]
        fn initial_liquidity_shares_use_integer_square_root() {
            assert_eq!(integer_sqrt_product(100, 400), Ok(200));
            assert_eq!(integer_sqrt_product(2, 2), Ok(2));
        }
    }
}
