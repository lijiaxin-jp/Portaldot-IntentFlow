use defi_domain::{Balance, FEE_DENOMINATOR_BPS, PoolSnapshot};
use ruint::aliases::{U256, U512};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HopQuote {
    pub amount_in: Balance,
    pub fee_paid: Balance,
    pub amount_out: Balance,
    pub reserve_in: Balance,
    pub reserve_out: Balance,
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum QuoteError {
    #[error("quote input must be greater than zero")]
    ZeroInput,
    #[error("pool reserves must be greater than zero")]
    ZeroLiquidity,
    #[error("fee {fee_bps} bps exceeds the denominator")]
    InvalidFee { fee_bps: u32 },
    #[error("quote arithmetic overflowed its checked integer width")]
    ArithmeticOverflow,
    #[error("quote output rounds down to zero")]
    ZeroOutput,
    #[error("input asset is not in the pool")]
    InvalidAsset,
    #[error("pool snapshot is invalid: {0}")]
    InvalidSnapshot(defi_domain::DomainError),
    #[error("quote output would consume the output reserve")]
    InsufficientOutputLiquidity,
}

pub fn quote_constant_product(
    reserve_in: Balance,
    reserve_out: Balance,
    amount_in: Balance,
    fee_bps: u32,
) -> Result<HopQuote, QuoteError> {
    if amount_in == 0 {
        return Err(QuoteError::ZeroInput);
    }
    if reserve_in == 0 || reserve_out == 0 {
        return Err(QuoteError::ZeroLiquidity);
    }
    if fee_bps > FEE_DENOMINATOR_BPS {
        return Err(QuoteError::InvalidFee { fee_bps });
    }
    let fee_factor = U512::from(FEE_DENOMINATOR_BPS - fee_bps);
    let fee_denominator = U512::from(FEE_DENOMINATOR_BPS);
    let amount_in_scaled = U512::from(amount_in)
        .checked_mul(fee_factor)
        .ok_or(QuoteError::ArithmeticOverflow)?;
    let numerator = amount_in_scaled
        .checked_mul(U512::from(reserve_out))
        .ok_or(QuoteError::ArithmeticOverflow)?;
    let denominator = U512::from(reserve_in)
        .checked_mul(fee_denominator)
        .and_then(|value| value.checked_add(amount_in_scaled))
        .ok_or(QuoteError::ArithmeticOverflow)?;
    if denominator == U512::ZERO {
        return Err(QuoteError::ArithmeticOverflow);
    }
    let amount_out =
        u128::try_from(numerator / denominator).map_err(|_| QuoteError::ArithmeticOverflow)?;
    if amount_out == 0 {
        return Err(QuoteError::ZeroOutput);
    }
    if amount_out >= reserve_out {
        return Err(QuoteError::InsufficientOutputLiquidity);
    }
    let fee_paid = u128::try_from(
        U512::from(amount_in)
            .checked_mul(U512::from(fee_bps))
            .ok_or(QuoteError::ArithmeticOverflow)?
            / fee_denominator,
    )
    .map_err(|_| QuoteError::ArithmeticOverflow)?;
    Ok(HopQuote {
        amount_in,
        fee_paid,
        amount_out,
        reserve_in,
        reserve_out,
    })
}

pub(crate) fn mul_div_floor(
    a: Balance,
    b: Balance,
    denominator: Balance,
) -> Result<Balance, QuoteError> {
    if denominator == 0 {
        return Err(QuoteError::ZeroLiquidity);
    }
    let quotient = U256::from(a)
        .checked_mul(U256::from(b))
        .ok_or(QuoteError::ArithmeticOverflow)?
        / U256::from(denominator);
    u128::try_from(quotient).map_err(|_| QuoteError::ArithmeticOverflow)
}

pub fn quote_pool_exact_in(
    pool: &PoolSnapshot,
    asset_in: defi_domain::AssetId,
    amount_in: Balance,
) -> Result<HopQuote, QuoteError> {
    pool.validate().map_err(QuoteError::InvalidSnapshot)?;
    let (reserve_in, reserve_out) = pool
        .reserves_for(asset_in)
        .ok_or(QuoteError::InvalidAsset)?;
    quote_constant_product(reserve_in, reserve_out, amount_in, pool.fee_bps)
}
