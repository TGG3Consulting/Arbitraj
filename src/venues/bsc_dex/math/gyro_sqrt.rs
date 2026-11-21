//! Файл: venues/bsc_dex/math/gyro_sqrt.rs
//! Что делает: решение 452 — порт GyroPoolMath.sqrt (корень фикс-точки 18 знаков методом Ньютона: начальное
//! приближение по двоичному логарифму / таблице, ровно 7 итераций, проверка допуска — иначе откат «_sqrt FAILED»).
//! Кто вызывает: math/gyro_2clp.rs, math/gyro_eclp.rs, math/gyro_eclp_inv.rs.
//! От чего зависит: math/balancer_math.rs (mulDown, mulUp, ONE).
//! Настройки: нет.
//!
//! Источник (решение 23): balancer/balancer-deployments@b55dc2b8 build-info v3/tasks/20260123-v3-gyro-2clp-v2 →
//! contracts/lib/GyroPoolMath.sol L12-L20 (константы √10^-k), sqrt L23-L53, _makeInitialGuess L55-L112,
//! _intLog2Halved L114-L143 (файл побайтно равен в 20260126-v3-gyro-eclp-v2 и выпусках 2025 года).

use alloy_primitives::U256;

use super::balancer_math::{mul_down, mul_up, one};

/// _intLog2Halved (L114-L143): половина целого двоичного логарифма. Ошибок нет.
fn log2_halved(mut x: U256) -> usize {
    let mut n = 0usize;
    // Сдвиг на 128, 64, …, 2 бита — к n прибавляется половина сдвига (L115-L142).
    for bits in [128usize, 64, 32, 16, 8, 4, 2] {
        if x >= U256::from(1u8) << bits {
            x >>= bits;
            n += bits / 2;
        }
    }
    n
}

/// _makeInitialGuess (L55-L112). Ошибок нет (переполнение невозможно: 2^n·1e18, n ≤ 127).
fn initial(input: U256) -> U256 {
    if input >= one() {
        return (U256::from(1u8) << log2_halved(input / one())) * one();
    }
    // (верхняя граница входа, приближение): L59-L110.
    const TABLE: [(u64, u64); 16] = [
        (10, 3_162_277_660),
        (100, 10_000_000_000),
        (1_000, 31_622_776_601),
        (10_000, 100_000_000_000),
        (100_000, 316_227_766_016),
        (1_000_000, 1_000_000_000_000),
        (10_000_000, 3_162_277_660_168),
        (100_000_000, 10_000_000_000_000),
        (1_000_000_000, 31_622_776_601_683),
        (10_000_000_000, 100_000_000_000_000),
        (100_000_000_000, 316_227_766_016_837),
        (1_000_000_000_000, 1_000_000_000_000_000),
        (10_000_000_000_000, 3_162_277_660_168_379),
        (100_000_000_000_000, 10_000_000_000_000_000),
        (1_000_000_000_000_000, 31_622_776_601_683_793),
        (10_000_000_000_000_000, 100_000_000_000_000_000),
    ];
    for (lim, guess) in TABLE {
        if input <= U256::from(lim) {
            return U256::from(guess);
        }
    }
    if input <= U256::from(100_000_000_000_000_000u64) {
        return U256::from(316_227_766_016_837_933u64);
    }
    input
}

/// GyroPoolMath.sqrt (L23-L53). Принимает: число (18 знаков), допуск. Возвращает: корень (18 знаков). None — откат
/// проверки допуска («_sqrt FAILED»), недобор input − допуск или переполнение.
pub fn sqrt(input: U256, tolerance: U256) -> Option<U256> {
    if input.is_zero() {
        return Some(U256::ZERO);
    }
    let mut guess = initial(input);
    let num = input.checked_mul(one())?;
    for _ in 0..7 {
        guess = guess.checked_add(num.checked_div(guess)?)? / U256::from(2u8);
    }
    let sq = mul_down(guess, guess)?;
    let tol = mul_up(guess, tolerance)?;
    (sq <= input.checked_add(tol)? && sq >= input.checked_sub(tol)?).then_some(guess)
}
