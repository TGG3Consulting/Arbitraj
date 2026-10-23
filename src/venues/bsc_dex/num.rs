//! Файл: venues/bsc_dex/num.rs
//! Что делает: перевод сумм блокчейна (целое U256 в минимальных единицах токена) в Decimal и обратно
//! с учётом decimals токена — без float и без потери точности.
//! Кто вызывает: wallet/balances.rs, quote/*, exec/*, check/*.
//! От чего зависит: alloy-primitives, rust_decimal.
//! Настройки: нет.
//!
//! decimals токена берётся из самого контракта (ERC-20 decimals(), https://eips.ethereum.org/EIPS/eip-20);
//! у BNB 18 знаков: 1 BNB = 10^18 wei — единица газа и баланса узла (eth_getBalance возвращает wei,
//! https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/state.yaml).

use alloy_primitives::U256;
use rust_decimal::Decimal;

/// Знаков после запятой у BNB (wei) — единица протокола, см. шапку файла.
pub const BNB_DECIMALS: u8 = 18;

/// Целое в минимальных единицах → Decimal.
/// Принимает: сумму, decimals. Возвращает: Decimal; None — не помещается в Decimal (больше 28 значащих цифр
/// или decimals больше 28).
pub fn to_decimal(v: U256, decimals: u8) -> Option<Decimal> {
    if u32::from(decimals) > Decimal::MAX_SCALE {
        return None;
    }
    let n: i128 = i128::try_from(v).ok()?;
    Decimal::try_from_i128_with_scale(n, u32::from(decimals)).ok()
}

/// Decimal → целое в минимальных единицах, остаток меньше единицы отбрасывается (округление вниз).
/// Принимает: неотрицательную сумму, decimals. Возвращает: U256; None — сумма отрицательная
/// или переполнение при умножении на 10^decimals.
pub fn to_units(v: Decimal, decimals: u8) -> Option<U256> {
    if v.is_sign_negative() {
        return None;
    }
    let scale = v.scale();
    let mantissa = U256::from(u128::try_from(v.mantissa()).ok()?);
    let d = u32::from(decimals);
    if d >= scale {
        mantissa.checked_mul(U256::from(10u8).checked_pow(U256::from(d - scale))?)
    } else {
        Some(mantissa / U256::from(10u8).checked_pow(U256::from(scale - d))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    /// Перевод туда и обратно без потерь; лишние знаки отбрасываются вниз.
    #[test]
    fn round_trip() {
        let wei = U256::from(1_234_500_000_000_000_000u128);
        let d = to_decimal(wei, BNB_DECIMALS).unwrap();
        assert_eq!(d, Decimal::from_str("1.2345").unwrap());
        assert_eq!(to_units(d, BNB_DECIMALS).unwrap(), wei);
        assert_eq!(
            to_units(Decimal::from_str("1.239").unwrap(), 2).unwrap(),
            U256::from(123u8)
        );
        assert!(to_units(Decimal::from_str("-1").unwrap(), 2).is_none());
        assert!(to_decimal(U256::MAX, 18).is_none());
    }
}
