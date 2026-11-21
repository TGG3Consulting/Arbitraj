//! Файл: venues/bsc_dex/math/ten.rs
//! Что делает: степени десяти в U256, посчитанные при сборке (решение 410, задача t195). Раньше единицы фикс-точки
//! (1e18, 1e36, 1e10 …) считались `U256::from(10).pow(…)` при КАЖДОМ вызове — в том числе внутри циклов математики
//! пулов (Ньютон Curve/Solidly, бисекции глубины): perf 18.11 — ruint wrapping_pow 6,2 % времени процесса. Теперь —
//! готовые значения (константы E10…E36 и таблица TENS): значение то же самое (та же функция `pow` ruint, только
//! вычислена компилятором).
//! Кто вызывает: math/{curve_math, solidly_math, dodo_math, maverick_math, dmm, balancer_math, balancer_log_exp,
//! stableswap_v1}.rs, quote/{pmm_ladder, solidly_depth, tick_bin_spot, vault_depth, bound_rate, cycle_audit, cycle_cost,
//! cycle_rate}.rs, num.rs.
//! От чего зависит: alloy-primitives (ruint Uint::pow — const fn).
//! Настройки: нет (10 — основание десятичной записи, не параметр).

use alloy_primitives::U256;

/// Сколько степеней помещается в U256: 10^77 < 2^256 < 10^78.
const COUNT: usize = 78;

/// Что делает: 10^n той же функцией, что раньше в каждом вызове (`U256::from(10).pow(U256::from(n))`, с переносом по
/// модулю 2^256 при переполнении — как ruint wrapping_pow). Принимает: n. Возвращает: 10^n. Ошибок нет.
pub const fn pow10(n: u64) -> U256 {
    U256::from_limbs([10, 0, 0, 0]).pow(U256::from_limbs([n, 0, 0, 0]))
}

/// 10^0 … 10^77 — все степени десяти, которые помещаются в U256 без переполнения. static (одна копия в памяти,
/// ревью 410), значения посчитаны при сборке.
static TENS: [U256; COUNT] = {
    let mut t = [U256::ZERO; COUNT];
    let mut i = 0;
    while i < COUNT {
        t[i] = pow10(i as u64);
        i += 1;
    }
    t
};

/// 10^10.
pub const E10: U256 = pow10(10);
/// 10^17.
pub const E17: U256 = pow10(17);
/// 10^18.
pub const E18: U256 = pow10(18);
/// 10^36.
pub const E36: U256 = pow10(36);

/// Что делает: 10^n без переполнения — как `U256::from(10).checked_pow(U256::from(n))`: 10^78 и больше не помещаются
/// в U256 — None. Принимает: n. Возвращает: 10^n или None. Ошибок нет.
pub fn checked(n: u32) -> Option<U256> {
    TENS.get(n as usize).copied()
}

/// Что делает: 10^n — как `U256::from(10).pow(U256::from(n))` (перенос по модулю 2^256 при n ≥ 78): из таблицы, а
/// вне неё — тем же pow. Принимает: n. Возвращает: 10^n. Ошибок нет.
pub fn pow(n: u64) -> U256 {
    match usize::try_from(n).ok().and_then(|i| TENS.get(i)) {
        Some(v) => *v,
        None => pow10(n),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Таблица совпадает с прежним расчётом при каждом вызове: pow и checked_pow ruint.
    #[test]
    fn same_as_runtime_pow() {
        let ten = U256::from(10u8);
        for n in 0..90u32 {
            assert_eq!(checked(n), ten.checked_pow(U256::from(n)), "10^{n}");
            assert_eq!(pow10(u64::from(n)), ten.pow(U256::from(n)), "10^{n}");
            assert_eq!(pow(u64::from(n)), ten.pow(U256::from(n)), "10^{n}");
        }
        assert_eq!(E18, U256::from(1_000_000_000_000_000_000u64));
        assert_eq!(E36, E18 * E18);
        assert_eq!(E10, U256::from(10_000_000_000u64));
        assert_eq!(E17 * ten, E18);
    }
}
