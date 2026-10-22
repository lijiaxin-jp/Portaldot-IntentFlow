#![cfg_attr(not(feature = "std"), no_std, no_main)]

#[ink::contract]
mod intent_router {
    use ink::codegen::TraitCallBuilder;
    use ink::storage::Mapping;
    use intentflow_contract_interface::{
        account_is_zero, hash_execution_commitment, hash_route, AmmPool, AssetId, ContractError,
        ExecutionResult, IntentRouter, Psp22, RouteStep, SwapIntent,
    };

    const NETWORK_ID: &str = "portaldot-testnet";
    const MAX_ROUTE_AGE: u64 = 2;

    #[ink(storage)]
    pub struct IntentRouterContract {
        admin: AccountId,
        paused: bool,
        allowed_pools: Mapping<AccountId, bool>,
        authorized_solvers: Mapping<AccountId, bool>,
        executed_intents: Mapping<Hash, bool>,
        entered: bool,
    }

    #[ink(event)]
    pub struct IntentSubmitted {
        #[ink(topic)]
        intent_id: Hash,
        #[ink(topic)]
        owner: AccountId,
        asset_in: AssetId,
        asset_out: AssetId,
        amount_in: Balance,
    }

    #[ink(event)]
    pub struct RouteExecuted {
        #[ink(topic)]
        intent_id: Hash,
        #[ink(topic)]
        route_hash: Hash,
        amount_in: Balance,
        amount_out: Balance,
        fee: Balance,
        solver: AccountId,
    }

    impl IntentRouterContract {
        #[ink(constructor)]
        pub fn new() -> Result<Self, ContractError> {
            let admin = Self::env().caller();
            if account_is_zero(admin) {
                return Err(ContractError::ZeroAddress);
            }
            Ok(Self {
                admin,
                paused: false,
                allowed_pools: Mapping::default(),
                authorized_solvers: Mapping::default(),
                executed_intents: Mapping::default(),
                entered: false,
            })
        }

        fn revert(error: ContractError) -> ! {
            ink::env::return_value(
                ink::env::ReturnFlags::REVERT,
                &Result::<(), ContractError>::Err(error),
            )
        }

        fn token_transfer_from(
            token_account: AccountId,
            from: AccountId,
            to: AccountId,
            amount: Balance,
        ) -> Result<(), ContractError> {
            let mut token: ink::contract_ref!(Psp22) = token_account.into();
            match token
                .call_mut()
                .transfer_from(from, to, amount)
                .try_invoke()
            {
                Ok(Ok(Ok(()))) => Ok(()),
                _ => Err(ContractError::TokenCallFailed),
            }
        }

        fn token_approve(
            token_account: AccountId,
            spender: AccountId,
            amount: Balance,
        ) -> Result<(), ContractError> {
            let mut token: ink::contract_ref!(Psp22) = token_account.into();
            match token.call_mut().approve(spender, amount).try_invoke() {
                Ok(Ok(Ok(()))) => Ok(()),
                _ => Err(ContractError::TokenCallFailed),
            }
        }

        fn pool_tokens(pool_id: AccountId) -> Result<(AccountId, AccountId), ContractError> {
            let pool: ink::contract_ref!(AmmPool) = pool_id.into();
            let token_0 = match pool.call().token_0().try_invoke() {
                Ok(Ok(token)) => token,
                _ => return Err(ContractError::PoolCallFailed),
            };
            let token_1 = match pool.call().token_1().try_invoke() {
                Ok(Ok(token)) => token,
                _ => return Err(ContractError::PoolCallFailed),
            };
            Ok((token_0, token_1))
        }

        fn pool_quote(
            pool_id: AccountId,
            asset_in: AccountId,
            amount_in: Balance,
        ) -> Result<Balance, ContractError> {
            let pool: ink::contract_ref!(AmmPool) = pool_id.into();
            match pool
                .call()
                .quote_exact_input(asset_in, amount_in)
                .try_invoke()
            {
                Ok(Ok(Ok(amount))) => Ok(amount),
                Ok(Ok(Err(error))) => Err(error),
                _ => Err(ContractError::PoolCallFailed),
            }
        }

        fn pool_quote_without_fee(
            pool_id: AccountId,
            asset_in: AccountId,
            amount_in: Balance,
        ) -> Result<Balance, ContractError> {
            let pool: ink::contract_ref!(AmmPool) = pool_id.into();
            match pool
                .call()
                .quote_without_fee(asset_in, amount_in)
                .try_invoke()
            {
                Ok(Ok(Ok(amount))) => Ok(amount),
                Ok(Ok(Err(error))) => Err(error),
                _ => Err(ContractError::PoolCallFailed),
            }
        }

        fn validate_step(step: RouteStep) -> Result<(), ContractError> {
            let (token_0, token_1) = Self::pool_tokens(step.pool_id)?;
            if !((step.asset_in == AssetId::Contract(token_0)
                && step.asset_out == AssetId::Contract(token_1))
                || (step.asset_in == AssetId::Contract(token_1)
                    && step.asset_out == AssetId::Contract(token_0)))
            {
                return Err(ContractError::InvalidRoute);
            }
            Ok(())
        }

        fn validate_deadline(now: u64, deadline: u64) -> Result<(), ContractError> {
            if deadline < now {
                return Err(ContractError::InvalidDeadline);
            }
            Ok(())
        }

        fn validate_quote_constraints(
            amount_out: Balance,
            fee: Balance,
            amount_out_min: Balance,
            max_fee: Balance,
        ) -> Result<(), ContractError> {
            if amount_out < amount_out_min {
                return Err(ContractError::SlippageExceeded);
            }
            if fee > max_fee {
                return Err(ContractError::FeeLimitExceeded);
            }
            Ok(())
        }

        fn validate_route_shape(
            intent: &SwapIntent,
            steps: &[RouteStep],
        ) -> Result<(), ContractError> {
            if steps.is_empty() || steps.len() > 2 {
                return Err(ContractError::InvalidRoute);
            }
            let first_step = steps.first().ok_or(ContractError::InvalidRoute)?;
            let last_step = steps.last().ok_or(ContractError::InvalidRoute)?;
            if first_step.asset_in != intent.asset_in || last_step.asset_out != intent.asset_out {
                return Err(ContractError::InvalidRoute);
            }
            for (index, step) in steps.iter().enumerate() {
                if step.asset_in == step.asset_out || account_is_zero(step.pool_id) {
                    return Err(ContractError::InvalidRoute);
                }
                if let Some(previous_index) = index.checked_sub(1) {
                    if steps
                        .get(previous_index)
                        .is_some_and(|previous| previous.asset_out != step.asset_in)
                    {
                        return Err(ContractError::InvalidRoute);
                    }
                }
                if steps[..index]
                    .iter()
                    .any(|prior| prior.pool_id == step.pool_id)
                {
                    return Err(ContractError::InvalidRoute);
                }
            }
            Ok(())
        }

        fn validate_route(
            &self,
            intent: &SwapIntent,
            steps: &[RouteStep],
            snapshot_block_number: u64,
            snapshot_block_hash: Hash,
            expected_commitment: Hash,
        ) -> Result<Hash, ContractError> {
            if snapshot_block_number > self.env().block_number() as u64
                || (self.env().block_number() as u64).saturating_sub(snapshot_block_number)
                    > MAX_ROUTE_AGE
            {
                return Err(ContractError::StaleRoute);
            }
            Self::validate_deadline(self.env().block_number() as u64, intent.deadline)?;
            if intent.amount_in == 0 {
                return Err(ContractError::InvalidAmount);
            }
            if intent.asset_in == intent.asset_out {
                return Err(ContractError::InvalidAssetPair);
            }
            if account_is_zero(intent.owner) || intent.intent_id == Hash::from([0; 32]) {
                return Err(ContractError::ZeroAddress);
            }
            Self::validate_route_shape(intent, steps)?;
            for step in steps {
                if !self.is_pool_allowed(step.pool_id) {
                    return Err(ContractError::PoolNotAllowed);
                }
                Self::validate_step(*step)?;
            }
            let calculated_route_hash = hash_route(
                NETWORK_ID,
                snapshot_block_number,
                snapshot_block_hash,
                steps,
            );
            let commitment = hash_execution_commitment(intent, calculated_route_hash);
            if commitment != expected_commitment {
                return Err(ContractError::InvalidRoute);
            }
            Ok(calculated_route_hash)
        }

        fn quote_route(
            steps: &[RouteStep],
            amount_in: Balance,
        ) -> Result<(Balance, Balance), ContractError> {
            let mut amount_with_fee = amount_in;
            let mut amount_without_fee = amount_in;
            for step in steps {
                let asset_in = match step.asset_in {
                    AssetId::Contract(account) => account,
                    AssetId::Native => return Err(ContractError::InvalidAssetPair),
                };
                amount_with_fee = Self::pool_quote(step.pool_id, asset_in, amount_with_fee)?;
                amount_without_fee =
                    Self::pool_quote_without_fee(step.pool_id, asset_in, amount_without_fee)?;
            }
            Ok((
                amount_with_fee,
                amount_without_fee.saturating_sub(amount_with_fee),
            ))
        }

        fn execute_step(
            step: RouteStep,
            amount_in: Balance,
            amount_out_min: Balance,
            deadline: u64,
            recipient: AccountId,
        ) -> Result<Balance, ContractError> {
            let asset_in = match step.asset_in {
                AssetId::Contract(account) => account,
                AssetId::Native => return Err(ContractError::InvalidAssetPair),
            };
            Self::token_approve(asset_in, step.pool_id, amount_in)?;
            let mut pool: ink::contract_ref!(AmmPool) = step.pool_id.into();
            match pool
                .call_mut()
                .swap_exact_input(asset_in, amount_in, amount_out_min, deadline, recipient)
                .try_invoke()
            {
                Ok(Ok(Ok(amount))) => Ok(amount),
                Ok(Ok(Err(error))) => Err(error),
                _ => Err(ContractError::PoolCallFailed),
            }
        }
    }

    impl IntentRouter for IntentRouterContract {
        #[ink(message)]
        fn admin(&self) -> AccountId {
            self.admin
        }

        #[ink(message)]
        fn set_pool_allowed(
            &mut self,
            pool: AccountId,
            allowed: bool,
        ) -> Result<(), ContractError> {
            if self.env().caller() != self.admin {
                return Err(ContractError::Unauthorized);
            }
            if account_is_zero(pool) {
                return Err(ContractError::ZeroAddress);
            }
            self.allowed_pools.insert(pool, &allowed);
            Ok(())
        }

        #[ink(message)]
        fn is_pool_allowed(&self, pool: AccountId) -> bool {
            self.allowed_pools.get(pool).unwrap_or(false)
        }

        #[ink(message)]
        fn set_solver_authorized(
            &mut self,
            solver: AccountId,
            authorized: bool,
        ) -> Result<(), ContractError> {
            if self.env().caller() != self.admin {
                return Err(ContractError::Unauthorized);
            }
            if account_is_zero(solver) {
                return Err(ContractError::ZeroAddress);
            }
            self.authorized_solvers.insert(solver, &authorized);
            Ok(())
        }

        #[ink(message)]
        fn is_solver_authorized(&self, solver: AccountId) -> bool {
            self.authorized_solvers.get(solver).unwrap_or(false)
        }

        #[ink(message)]
        fn set_paused(&mut self, paused: bool) -> Result<(), ContractError> {
            if self.env().caller() != self.admin {
                return Err(ContractError::Unauthorized);
            }
            self.paused = paused;
            Ok(())
        }

        #[ink(message)]
        fn is_paused(&self) -> bool {
            self.paused
        }

        #[ink(message)]
        fn execute(
            &mut self,
            intent: SwapIntent,
            steps: ink::prelude::vec::Vec<RouteStep>,
            snapshot_block_number: u64,
            snapshot_block_hash: Hash,
            expected_commitment: Hash,
        ) -> Result<ExecutionResult, ContractError> {
            let solver = self.env().caller();
            if self.paused {
                return Err(ContractError::Paused);
            }
            if solver != intent.owner && !self.is_solver_authorized(solver) {
                return Err(ContractError::Unauthorized);
            }
            if self.executed_intents.get(intent.intent_id).unwrap_or(false) {
                return Err(ContractError::InvalidRoute);
            }
            if self.entered {
                return Err(ContractError::ReentrantCall);
            }
            self.entered = true;
            let preflight = (|| {
                let route_hash = self.validate_route(
                    &intent,
                    &steps,
                    snapshot_block_number,
                    snapshot_block_hash,
                    expected_commitment,
                )?;
                let (quoted_output, fee) = Self::quote_route(&steps, intent.amount_in)?;
                Self::validate_quote_constraints(
                    quoted_output,
                    fee,
                    intent.amount_out_min,
                    intent.max_fee,
                )?;
                let input_token = match intent.asset_in {
                    AssetId::Contract(account) => account,
                    AssetId::Native => return Err(ContractError::InvalidAssetPair),
                };
                Ok((route_hash, quoted_output, fee, input_token))
            })();
            let (route_hash, _quoted_output, fee, input_token) = match preflight {
                Ok(values) => values,
                Err(error) => {
                    self.entered = false;
                    return Err(error);
                }
            };
            self.executed_intents.insert(intent.intent_id, &true);
            if Self::token_transfer_from(
                input_token,
                intent.owner,
                self.env().account_id(),
                intent.amount_in,
            )
            .is_err()
            {
                Self::revert(ContractError::TokenCallFailed);
            }
            self.env().emit_event(IntentSubmitted {
                intent_id: intent.intent_id,
                owner: intent.owner,
                asset_in: intent.asset_in,
                asset_out: intent.asset_out,
                amount_in: intent.amount_in,
            });

            let mut amount = intent.amount_in;
            let last_step_index = steps.len().saturating_sub(1);
            for (index, step) in steps.iter().enumerate() {
                let is_last_step = index == last_step_index;
                let recipient = if is_last_step {
                    intent.owner
                } else {
                    self.env().account_id()
                };
                let min_output = if is_last_step {
                    intent.amount_out_min
                } else {
                    1
                };
                amount =
                    match Self::execute_step(*step, amount, min_output, intent.deadline, recipient)
                    {
                        Ok(value) => value,
                        Err(error) => Self::revert(error),
                    };
            }
            if amount < intent.amount_out_min {
                Self::revert(ContractError::SlippageExceeded);
            }
            if fee > intent.max_fee {
                Self::revert(ContractError::FeeLimitExceeded);
            }
            self.entered = false;
            self.env().emit_event(RouteExecuted {
                intent_id: intent.intent_id,
                route_hash,
                amount_in: intent.amount_in,
                amount_out: amount,
                fee,
                solver,
            });
            Ok(ExecutionResult {
                amount_in: intent.amount_in,
                amount_out: amount,
                fee,
                route_hash,
                solver,
            })
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn account(byte: u8) -> AccountId {
            AccountId::from([byte; 32])
        }

        fn intent() -> SwapIntent {
            SwapIntent {
                intent_id: Hash::from([9; 32]),
                owner: account(1),
                asset_in: AssetId::Contract(account(4)),
                asset_out: AssetId::Contract(account(6)),
                amount_in: 100,
                amount_out_min: 80,
                max_fee: 10,
                deadline: 10,
            }
        }

        fn two_hop_route() -> ink::prelude::vec::Vec<RouteStep> {
            ink::prelude::vec![
                RouteStep {
                    pool_id: account(2),
                    asset_in: AssetId::Contract(account(4)),
                    asset_out: AssetId::Contract(account(5)),
                },
                RouteStep {
                    pool_id: account(3),
                    asset_in: AssetId::Contract(account(5)),
                    asset_out: AssetId::Contract(account(6)),
                },
            ]
        }

        #[ink::test]
        fn admin_controls_pool_solver_allowlists_and_pause() {
            let mut router = IntentRouterContract::new().expect("constructor should succeed");
            assert_eq!(router.set_pool_allowed(account(2), true), Ok(()));
            assert_eq!(router.set_solver_authorized(account(3), true), Ok(()));
            assert!(router.is_pool_allowed(account(2)));
            assert!(router.is_solver_authorized(account(3)));
            assert_eq!(router.set_paused(true), Ok(()));
            assert!(router.is_paused());
        }

        #[ink::test]
        fn non_admin_cannot_change_route_policy() {
            let mut router = IntentRouterContract::new().expect("constructor should succeed");
            ink::env::test::set_caller::<ink::env::DefaultEnvironment>(account(2));
            assert_eq!(
                router.set_pool_allowed(account(3), true),
                Err(ContractError::Unauthorized)
            );
            assert_eq!(
                router.set_solver_authorized(account(3), true),
                Err(ContractError::Unauthorized)
            );
            assert_eq!(router.set_paused(true), Err(ContractError::Unauthorized));
        }

        #[ink::test]
        fn route_hash_commits_to_network_block_and_every_hop() {
            let steps = ink::prelude::vec![
                RouteStep {
                    pool_id: account(3),
                    asset_in: AssetId::Contract(account(1)),
                    asset_out: AssetId::Contract(account(2))
                },
                RouteStep {
                    pool_id: account(4),
                    asset_in: AssetId::Contract(account(2)),
                    asset_out: AssetId::Contract(account(1))
                },
            ];
            let first = hash_route(NETWORK_ID, 10, [8; 32].into(), &steps);
            assert_ne!(first, hash_route(NETWORK_ID, 11, [8; 32].into(), &steps));
            assert_ne!(first, hash_route(NETWORK_ID, 10, [9; 32].into(), &steps));
            assert_ne!(
                first,
                hash_route("other-network", 10, [8; 32].into(), &steps)
            );
            let mut changed = steps.clone();
            changed[1].pool_id = account(5);
            assert_ne!(first, hash_route(NETWORK_ID, 10, [8; 32].into(), &changed));
        }

        #[ink::test]
        fn route_shape_allows_one_or_two_hops_and_rejects_disconnected_or_reused_pools() {
            let intent = intent();
            let route = two_hop_route();
            assert_eq!(
                IntentRouterContract::validate_route_shape(&intent, &route),
                Ok(())
            );
            let one_hop = [RouteStep {
                pool_id: account(2),
                asset_in: intent.asset_in,
                asset_out: intent.asset_out,
            }];
            assert_eq!(
                IntentRouterContract::validate_route_shape(&intent, &one_hop),
                Ok(())
            );
            assert_eq!(
                IntentRouterContract::validate_route_shape(&intent, &route[..0]),
                Err(ContractError::InvalidRoute)
            );
            let mut disconnected = route.clone();
            disconnected[1].asset_in = AssetId::Contract(account(7));
            assert_eq!(
                IntentRouterContract::validate_route_shape(&intent, &disconnected),
                Err(ContractError::InvalidRoute)
            );
            let mut reused = route;
            reused[1].pool_id = reused[0].pool_id;
            assert_eq!(
                IntentRouterContract::validate_route_shape(&intent, &reused),
                Err(ContractError::InvalidRoute)
            );
            let too_many = [one_hop[0], one_hop[0], one_hop[0]];
            assert_eq!(
                IntentRouterContract::validate_route_shape(&intent, &too_many),
                Err(ContractError::InvalidRoute)
            );
        }

        #[ink::test]
        fn quote_enforces_slippage_and_fee_limits() {
            assert_eq!(
                IntentRouterContract::validate_quote_constraints(79, 5, 80, 10),
                Err(ContractError::SlippageExceeded)
            );
            assert_eq!(
                IntentRouterContract::validate_quote_constraints(80, 11, 80, 10),
                Err(ContractError::FeeLimitExceeded)
            );
            assert_eq!(
                IntentRouterContract::validate_quote_constraints(80, 10, 80, 10),
                Ok(())
            );
        }

        #[ink::test]
        fn expired_deadline_and_non_whitelisted_pool_are_rejected_before_pool_calls() {
            let router = IntentRouterContract::new().expect("constructor should succeed");
            ink::env::test::set_block_number::<ink::env::DefaultEnvironment>(2);
            let mut expired = intent();
            expired.deadline = 1;
            assert_eq!(
                router.validate_route(
                    &expired,
                    &two_hop_route(),
                    2,
                    [8; 32].into(),
                    [0; 32].into()
                ),
                Err(ContractError::InvalidDeadline)
            );

            let valid = intent();
            assert_eq!(
                router.validate_route(&valid, &two_hop_route(), 2, [8; 32].into(), [0; 32].into()),
                Err(ContractError::PoolNotAllowed)
            );
        }
    }
}
