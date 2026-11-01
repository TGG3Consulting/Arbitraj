//! Файл: venues/kyberswap/num.rs
//! Что делает: перевод сумм блокчейна (целое U256 в минимальных единицах токена) в Decimal и обратно с учётом
//! decimals токена — без float и без потери точности. Своя копия для адаптера kyberswap (изоляция: не импортирует
//! bsc_dex).
//! Кто вызывает: quote.rs, trade.rs, balances.rs, transfer.rs, rest.rs.
//! От чего зависит: alloy-primitives, rust_decimal.
//! Настройки: нет.
//!
//! decimals токена — из контракта (ERC-20 decimals(), https://eips.ethereum.org/EIPS/eip-20); у BNB 18 знаков
//! (eth_getBalance возвращает wei).

use alloy_primitives::U256;
use rust_decimal::Decimal;

/// Знаков после запятой у BNB (wei) — единица протокола.
pub const BNB_DECIMALS: u8 = 18;

/// Целое в минимальных единицах → Decimal (младшие знаки отбрасываются вниз, если не помещается).
/// Принимает: сумму, decimals. Возвращает: Decimal; None — decimals больше 28 или число слишком велико.
pub fn to_decimal(v: U256, decimals: u8) -> Option<Decimal> {
    let mut scale = u32::from(decimals);
    if scale > Decimal::MAX_SCALE {
        return None;
    }
    let max = U256::from(u128::MAX >> 32);
    let ten = U256::from(10u8);
    let mut v = v;
    while v > max {
        if scale == 0 {
            return None;
        }
        v /= ten;
        scale -= 1;
    }
    Decimal::try_from_i128_with_scale(i128::try_from(v).ok()?, scale).ok()
}

/// Decimal → целое в минимальных единицах, остаток меньше единицы отбрасывается (вниз).
/// Принимает: неотрицательную сумму, decimals. Возвращает: U256; None — сумма отрицательная или переполнение.
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
