use defi_domain::{Balance, Hash};
use thiserror::Error;

const BPS_DENOMINATOR: Balance = 10_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PaperAccountConfig {
    pub initial_balance: Balance,
    pub max_trade_fraction_bps: u32,
    pub min_net_profit: Balance,
    pub cooldown_blocks: u64,
    pub charge_reverted_fee: bool,
}

impl PaperAccountConfig {
    pub fn validate(self) -> Result<(), PaperError> {
        if self.max_trade_fraction_bps == 0 || self.max_trade_fraction_bps > 10_000 {
            return Err(PaperError::InvalidTradeFraction);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaperCandidate {
    pub intent_id: Option<Hash>,
    pub amount_in: Balance,
    pub simulated_amount_out: Option<Balance>,
    pub fee_total: Balance,
    pub net_profit: Option<i128>,
    pub trace_hash: Option<Hash>,
    pub reverted: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PaperDecision {
    Accepted,
    Rejected,
    Reverted,
    Skipped,
}

impl PaperDecision {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
            Self::Reverted => "reverted",
            Self::Skipped => "skipped",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaperDecisionResult {
    pub intent_id: Option<Hash>,
    pub decision: PaperDecision,
    pub reason: &'static str,
    pub balance_before: Balance,
    pub amount_in: Balance,
    pub simulated_amount_out: Option<Balance>,
    pub fee_total: Balance,
    pub net_profit: Option<i128>,
    pub balance_after: Balance,
    pub trace_hash: Option<Hash>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PaperEquity {
    pub initial_balance: Balance,
    pub balance: Balance,
    pub cumulative_net_profit: i128,
    pub peak_balance: Balance,
    pub drawdown: Balance,
    pub accepted_count: u64,
    pub decision_count: u64,
    pub total_fees: Balance,
    pub gross_profit: Balance,
}

impl PaperEquity {
    #[must_use]
    pub fn win_rate_bps(self) -> u64 {
        if self.decision_count == 0 {
            0
        } else {
            self.accepted_count.saturating_mul(10_000) / self.decision_count
        }
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum PaperError {
    #[error("paper trade fraction must be between 1 and 10000 bps")]
    InvalidTradeFraction,
    #[error("paper account arithmetic overflow")]
    ArithmeticOverflow,
}

#[derive(Clone, Debug)]
pub struct PaperAccount {
    config: PaperAccountConfig,
    balance: Balance,
    peak_balance: Balance,
    cumulative_net_profit: i128,
    accepted_count: u64,
    decision_count: u64,
    total_fees: Balance,
    gross_profit: Balance,
    last_accepted_block: Option<u64>,
}

impl PaperAccount {
    pub fn new(config: PaperAccountConfig) -> Result<Self, PaperError> {
        config.validate()?;
        Ok(Self {
            balance: config.initial_balance,
            peak_balance: config.initial_balance,
            cumulative_net_profit: 0,
            accepted_count: 0,
            decision_count: 0,
            total_fees: 0,
            gross_profit: 0,
            last_accepted_block: None,
            config,
        })
    }

    pub fn decide(
        &mut self,
        block_number: u64,
        candidate: Option<PaperCandidate>,
    ) -> Result<PaperDecisionResult, PaperError> {
        let Some(candidate) = candidate else {
            self.decision_count = self.decision_count.saturating_add(1);
            return Ok(PaperDecisionResult {
                intent_id: None,
                decision: PaperDecision::Skipped,
                reason: "no_candidate",
                balance_before: self.balance,
                amount_in: 0,
                simulated_amount_out: None,
                fee_total: 0,
                net_profit: None,
                balance_after: self.balance,
                trace_hash: None,
            });
        };
        let balance_before = self.balance;
        let max_amount = balance_before
            .checked_mul(Balance::from(self.config.max_trade_fraction_bps))
            .and_then(|value| value.checked_div(BPS_DENOMINATOR))
            .ok_or(PaperError::ArithmeticOverflow)?;
        let cooldown_active = self
            .last_accepted_block
            .is_some_and(|last| block_number.saturating_sub(last) <= self.config.cooldown_blocks);
        if candidate.reverted {
            self.decision_count = self.decision_count.saturating_add(1);
            if self.config.charge_reverted_fee {
                self.balance = self
                    .balance
                    .checked_sub(candidate.fee_total)
                    .ok_or(PaperError::ArithmeticOverflow)?;
            }
            self.total_fees = self
                .total_fees
                .checked_add(candidate.fee_total)
                .ok_or(PaperError::ArithmeticOverflow)?;
            return Ok(PaperDecisionResult {
                intent_id: candidate.intent_id,
                decision: PaperDecision::Reverted,
                reason: "simulation_reverted",
                balance_before,
                amount_in: candidate.amount_in,
                simulated_amount_out: None,
                fee_total: candidate.fee_total,
                net_profit: None,
                balance_after: self.balance,
                trace_hash: candidate.trace_hash,
            });
        }
        let reason = if candidate.amount_in > max_amount {
            "trade_fraction_exceeded"
        } else if candidate.amount_in > balance_before {
            "insufficient_balance"
        } else if cooldown_active {
            "cooldown_active"
        } else if candidate.simulated_amount_out.is_none() || candidate.net_profit.is_none() {
            "missing_simulation_output"
        } else if candidate.net_profit.is_some_and(|profit| {
            i128::try_from(self.config.min_net_profit).map_or(true, |minimum| profit < minimum)
        }) {
            "minimum_profit_not_met"
        } else {
            "accepted"
        };
        let accepted = reason == "accepted";
        self.decision_count = self.decision_count.saturating_add(1);
        if accepted {
            let net_profit = candidate.net_profit.ok_or(PaperError::ArithmeticOverflow)?;
            self.balance = if net_profit >= 0 {
                self.balance
                    .checked_add(
                        Balance::try_from(net_profit)
                            .map_err(|_| PaperError::ArithmeticOverflow)?,
                    )
                    .ok_or(PaperError::ArithmeticOverflow)?
            } else {
                let loss = Balance::try_from(
                    net_profit
                        .checked_neg()
                        .ok_or(PaperError::ArithmeticOverflow)?,
                )
                .map_err(|_| PaperError::ArithmeticOverflow)?;
                self.balance
                    .checked_sub(loss)
                    .ok_or(PaperError::ArithmeticOverflow)?
            };
            self.cumulative_net_profit = self
                .cumulative_net_profit
                .checked_add(net_profit)
                .ok_or(PaperError::ArithmeticOverflow)?;
            self.gross_profit = self
                .gross_profit
                .checked_add(
                    Balance::try_from(net_profit).map_err(|_| PaperError::ArithmeticOverflow)?,
                )
                .and_then(|value| value.checked_add(candidate.fee_total))
                .ok_or(PaperError::ArithmeticOverflow)?;
            self.accepted_count = self.accepted_count.saturating_add(1);
            self.last_accepted_block = Some(block_number);
            self.peak_balance = self.peak_balance.max(self.balance);
        }
        self.total_fees = self
            .total_fees
            .checked_add(candidate.fee_total)
            .ok_or(PaperError::ArithmeticOverflow)?;
        Ok(PaperDecisionResult {
            intent_id: candidate.intent_id,
            decision: if accepted {
                PaperDecision::Accepted
            } else {
                PaperDecision::Rejected
            },
            reason,
            balance_before,
            amount_in: candidate.amount_in,
            simulated_amount_out: candidate.simulated_amount_out,
            fee_total: candidate.fee_total,
            net_profit: candidate.net_profit,
            balance_after: self.balance,
            trace_hash: candidate.trace_hash,
        })
    }

    #[must_use]
    pub fn equity(&self) -> PaperEquity {
        PaperEquity {
            initial_balance: self.config.initial_balance,
            balance: self.balance,
            cumulative_net_profit: self.cumulative_net_profit,
            peak_balance: self.peak_balance,
            drawdown: self.peak_balance.saturating_sub(self.balance),
            accepted_count: self.accepted_count,
            decision_count: self.decision_count,
            total_fees: self.total_fees,
            gross_profit: self.gross_profit,
        }
    }
}
