#![cfg_attr(not(feature = "std"), no_std, no_main)]

#[ink::contract]
mod p0_probe {
    #[ink(storage)]
    pub struct P0Probe {}

    #[ink(event)]
    pub struct ProbePinged {}

    impl P0Probe {
        #[ink(constructor)]
        pub fn new() -> Self {
            Self {}
        }

        #[ink(message)]
        pub fn get_sequence(&self) -> u32 {
            0
        }

        #[ink(message)]
        pub fn ping(&mut self) -> u32 {
            self.env().emit_event(ProbePinged {});
            0
        }
    }
}
