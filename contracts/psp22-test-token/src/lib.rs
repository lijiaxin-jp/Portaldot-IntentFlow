#![cfg_attr(not(feature = "std"), no_std, no_main)]

#[ink::contract]
mod psp22_test_token {
    use ink::prelude::string::String;
    use ink::storage::Mapping;
    use intentflow_contract_interface::{ContractError, Psp22};

    #[ink(storage)]
    pub struct Psp22TestToken {
        name: String,
        symbol: String,
        decimals: u8,
        admin: AccountId,
        total_supply: Balance,
        balances: Mapping<AccountId, Balance>,
        allowances: Mapping<(AccountId, AccountId), Balance>,
    }

    #[ink(event)]
    pub struct Transfer {
        #[ink(topic)]
        from: Option<AccountId>,
        #[ink(topic)]
        to: Option<AccountId>,
        amount: Balance,
    }

    #[ink(event)]
    pub struct Approval {
        #[ink(topic)]
        owner: AccountId,
        #[ink(topic)]
        spender: AccountId,
        amount: Balance,
    }

    impl Psp22TestToken {
        #[ink(constructor)]
        pub fn new(
            name: String,
            symbol: String,
            decimals: u8,
            initial_supply: Balance,
        ) -> Result<Self, ContractError> {
            let admin = Self::env().caller();
            if admin == AccountId::from([0; 32]) {
                return Err(ContractError::ZeroAddress);
            }
            let mut token = Self {
                name,
                symbol,
                decimals,
                admin,
                total_supply: 0,
                balances: Mapping::default(),
                allowances: Mapping::default(),
            };
            if initial_supply > 0 {
                token.mint_to(admin, initial_supply)?;
            }
            Ok(token)
        }

        #[ink(message)]
        pub fn admin(&self) -> AccountId {
            self.admin
        }

        #[ink(message)]
        pub fn mint(&mut self, to: AccountId, amount: Balance) -> Result<(), ContractError> {
            if self.env().caller() != self.admin {
                return Err(ContractError::Unauthorized);
            }
            self.mint_to(to, amount)
        }

        fn mint_to(&mut self, to: AccountId, amount: Balance) -> Result<(), ContractError> {
            if to == AccountId::from([0; 32]) {
                return Err(ContractError::ZeroAddress);
            }
            if amount == 0 {
                return Err(ContractError::InvalidAmount);
            }
            let balance = self.balance_of(to);
            let new_balance = balance
                .checked_add(amount)
                .ok_or(ContractError::ArithmeticOverflow)?;
            let new_supply = self
                .total_supply
                .checked_add(amount)
                .ok_or(ContractError::ArithmeticOverflow)?;
            self.total_supply = new_supply;
            self.balances.insert(to, &new_balance);
            self.env().emit_event(Transfer {
                from: None,
                to: Some(to),
                amount,
            });
            Ok(())
        }

        fn move_balance(
            &mut self,
            from: AccountId,
            to: AccountId,
            amount: Balance,
        ) -> Result<(), ContractError> {
            if to == AccountId::from([0; 32]) {
                return Err(ContractError::ZeroAddress);
            }
            let from_balance = self.balance_of(from);
            if from_balance < amount {
                return Err(ContractError::InsufficientBalance);
            }
            let new_from_balance = from_balance
                .checked_sub(amount)
                .ok_or(ContractError::InsufficientBalance)?;
            if from != to {
                let to_balance = self.balance_of(to);
                let new_to_balance = to_balance
                    .checked_add(amount)
                    .ok_or(ContractError::ArithmeticOverflow)?;
                self.balances.insert(from, &new_from_balance);
                self.balances.insert(to, &new_to_balance);
            }
            self.env().emit_event(Transfer {
                from: Some(from),
                to: Some(to),
                amount,
            });
            Ok(())
        }
    }

    impl Psp22 for Psp22TestToken {
        #[ink(message)]
        fn name(&self) -> String {
            self.name.clone()
        }

        #[ink(message)]
        fn symbol(&self) -> String {
            self.symbol.clone()
        }

        #[ink(message)]
        fn decimals(&self) -> u8 {
            self.decimals
        }

        #[ink(message)]
        fn test_only(&self) -> bool {
            true
        }

        #[ink(message)]
        fn total_supply(&self) -> Balance {
            self.total_supply
        }

        #[ink(message)]
        fn balance_of(&self, owner: AccountId) -> Balance {
            self.balances.get(owner).unwrap_or_default()
        }

        #[ink(message)]
        fn allowance(&self, owner: AccountId, spender: AccountId) -> Balance {
            self.allowances.get((owner, spender)).unwrap_or_default()
        }

        #[ink(message)]
        fn transfer(&mut self, to: AccountId, amount: Balance) -> Result<(), ContractError> {
            self.move_balance(self.env().caller(), to, amount)
        }

        #[ink(message)]
        fn approve(&mut self, spender: AccountId, amount: Balance) -> Result<(), ContractError> {
            if spender == AccountId::from([0; 32]) {
                return Err(ContractError::ZeroAddress);
            }
            let owner = self.env().caller();
            self.allowances.insert((owner, spender), &amount);
            self.env().emit_event(Approval {
                owner,
                spender,
                amount,
            });
            Ok(())
        }

        #[ink(message)]
        fn transfer_from(
            &mut self,
            from: AccountId,
            to: AccountId,
            amount: Balance,
        ) -> Result<(), ContractError> {
            let spender = self.env().caller();
            let allowed = self.allowance(from, spender);
            if allowed < amount {
                return Err(ContractError::InsufficientAllowance);
            }
            self.move_balance(from, to, amount)?;
            let remaining = allowed
                .checked_sub(amount)
                .ok_or(ContractError::InsufficientAllowance)?;
            self.allowances.insert((from, spender), &remaining);
            Ok(())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use ink::env::test;

        fn account(byte: u8) -> AccountId {
            AccountId::from([byte; 32])
        }

        #[ink::test]
        fn constructor_marks_test_token_and_mints_initial_supply() {
            let admin = test::default_accounts::<ink::env::DefaultEnvironment>().alice;
            let token = Psp22TestToken::new("Test A".into(), "TSTA".into(), 12, 1_000)
                .expect("constructor should accept valid supply");
            assert_eq!(token.admin(), admin);
            assert!(token.test_only());
            assert_eq!(token.total_supply(), 1_000);
            assert_eq!(token.balance_of(admin), 1_000);
        }

        #[ink::test]
        fn only_admin_can_mint() {
            let mut token = Psp22TestToken::new("Test A".into(), "TSTA".into(), 12, 10)
                .expect("constructor should succeed");
            test::set_caller::<ink::env::DefaultEnvironment>(account(2));
            assert_eq!(token.mint(account(3), 1), Err(ContractError::Unauthorized));
            assert_eq!(token.total_supply(), 10);
        }

        #[ink::test]
        fn transfer_and_transfer_from_update_balances_and_allowance() {
            let admin = test::default_accounts::<ink::env::DefaultEnvironment>().alice;
            let spender = account(2);
            let recipient = account(3);
            let mut token = Psp22TestToken::new("Test A".into(), "TSTA".into(), 12, 100)
                .expect("constructor should succeed");
            token
                .approve(spender, 40)
                .expect("approval should be accepted");
            test::set_caller::<ink::env::DefaultEnvironment>(spender);
            token
                .transfer_from(admin, recipient, 25)
                .expect("approved transfer should succeed");
            assert_eq!(token.balance_of(admin), 75);
            assert_eq!(token.balance_of(recipient), 25);
            assert_eq!(token.allowance(admin, spender), 15);
        }

        #[ink::test]
        fn failed_transfer_does_not_change_supply_or_balances() {
            let mut token = Psp22TestToken::new("Test A".into(), "TSTA".into(), 12, 10)
                .expect("constructor should succeed");
            assert_eq!(
                token.transfer(account(3), 11),
                Err(ContractError::InsufficientBalance)
            );
            assert_eq!(token.balance_of(account(3)), 0);
            assert_eq!(token.total_supply(), 10);
        }
    }
}
