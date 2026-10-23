//! Файл: venues/bsc_dex/math/v2.rs
//! Что делает: формулы пары V2 — выход по входу (getAmountOut) и вход по выходу (getAmountIn) с комиссией пула.
//! Кто вызывает: quote/pool_quote.rs.
//! От чего зависит: alloy-primitives, rust_decimal.
//! Настройки: комиссию даёт вызывающий — dex.v2_fee_pct [вид пула V2] («из кода пары своей DEX»).
//!
//! PancakeSwap: amountInWithFee = amountIn·9975; out = amountInWithFee·reserveOut / (reserveIn·10000 + amountInWithFee);
//! in = reserveIn·amountOut·10000 / ((reserveOut − amountOut)·9975) + 1 (PancakeLibrary.sol, research/flow/bsc_dex.md, раздел 3);
//! Uniswap: те же формулы с 997/1000 — https://github.com/Uniswap/v2-periphery/blob/ed24991304291297c3b4a52818d02f46a17aa9a2/contracts/libraries/UniswapV2Library.sol#L43-L60 .
//! Комиссия f% даёт дробь (100 − f)/100; умножение числителя и знаменателя на одно число деление вниз не меняет,
//! поэтому 9975/10000 и 997/1000 получаются из 0,25% и 0,3% точно.

use alloy_primitives::U256;
use rust_decimal::Decimal;

/// Дробь «доля после комиссии»: (числитель, знаменатель). None — комиссия вне 0–100% или не помещается.
pub fn fee_ratio(fee_pct: Decimal) -> Option<(U256, U256)> {
    if fee_pct.is_sign_negative() || fee_pct >= Decimal::ONE_HUNDRED {
        return None;
    }
    let scale = fee_pct.scale();
    let den = 100u128.checked_mul(10u128.checked_pow(scale)?)?;
    let fee_units = u128::try_from(fee_pct.mantissa()).ok()?;
    Some((U256::from(den - fee_units), U256::from(den)))
}

/// getAmountOut. None — нулевые резервы или вход.
pub fn amount_out(amount_in: U256, r_in: U256, r_out: U256, fee: (U256, U256)) -> Option<U256> {
    if amount_in.is_zero() || r_in.is_zero() || r_out.is_zero() {
        return None;
    }
    let with_fee = amount_in.checked_mul(fee.0)?;
    let num = with_fee.checked_mul(r_out)?;
    let den = r_in.checked_mul(fee.1)?.checked_add(with_fee)?;
    Some(num / den)
}

/// getAmountIn. None — нулевые резервы/выход или выход не меньше резерва (в паре столько нет).
pub fn amount_in(amount_out: U256, r_in: U256, r_out: U256, fee: (U256, U256)) -> Option<U256> {
    if amount_out.is_zero() || r_in.is_zero() || r_out <= amount_out {
        return None;
    }
    let num = r_in.checked_mul(amount_out)?.checked_mul(fee.1)?;
    let den = (r_out - amount_out).checked_mul(fee.0)?;
    Some(num / den + U256::from(1u8))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    /// 0,25% → 9975/10000 (PancakeSwap), 0,3% → 997/1000 (Uniswap) — те же значения, что в коде пар.
    #[test]
    fn fee_ratio_matches_contracts() {
        let (n, d) = fee_ratio(Decimal::from_str("0.25").unwrap()).unwrap();
        assert_eq!((n, d), (U256::from(9975u32), U256::from(10000u32)));
        let (n, d) = fee_ratio(Decimal::from_str("0.3").unwrap()).unwrap();
        assert_eq!((n, d), (U256::from(997u32), U256::from(1000u32)));
    }
}
