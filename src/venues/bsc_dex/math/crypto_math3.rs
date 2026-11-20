//! Файл: venues/bsc_dex/math/crypto_math3.rs
//! Что делает: решение 472 (T472-03) — `halfpow` и `sqrt_int` crypto-пулов Curve первого поколения и скользящее среднее
//! цены (price_oracle) на время блока, как его считает `tweak_price`: alpha = halfpow((t − last_prices_timestamp)·10^18 /
//! ma_half_time), oracle = (last_prices·(10^18 − alpha) + oracle·alpha) / 10^18. Все операции проверяемые (Vyper
//! 0.2.15 / 0.3.x): переполнение, деление на ноль, «Did not converge» — None.
//! Кто вызывает: pools/load_crypto_tweak_list.rs (оракул на время блока чтения), math/crypto_tweak_v1_3.rs (sqrt_int).
//! От чего зависит: math/crypto_ops.rs, math/ten.rs.
//! Настройки: нет.
//!
//! Источник: curvefi/curve-crypto-contract d7d04cd contracts/tricrypto/CurveCryptoMath3.vy — halfpow L253-L290,
//! sqrt_int L293-L310 (математика tricrypto Arbitrum 0x2F0AF8…9DaF, «2021-09-13. Arbitrum/log.txt»); тот же текст
//! halfpow с precision = EXP_PRECISION 10^10 — «2021-10-30. EURS/CryptoSwap.vy» L403-L436 и CurveCryptoSwap2ETH.vy
//! (curve-factory-crypto 7ed1011) L428-L460; вызов — tweak_price tricrypto L455-L457 (precision 10^10), EURS L586-L588.

use alloy_primitives::U256;

use super::crypto_ops::{au, du, mu, su};
use super::ten::{self, E10, E18};

/// `halfpow(power, precision)` (L253-L290): 10^18 · 0.5^(power/10^18). Принимает: степень, точность ряда. Возвращает:
/// значение или None (откат / «Did not converge»). Ошибок нет.
pub fn halfpow(power: U256, precision: U256) -> Option<U256> {
    let intpow = power / E18;
    let otherpow = power - intpow * E18;
    if intpow > U256::from(59u8) {
        return Some(U256::ZERO);
    }
    let result = E18 / (U256::from(1u8) << intpow.to::<usize>());
    if otherpow.is_zero() {
        return Some(result);
    }
    let x = U256::from(5u8) * ten::pow(17);
    let (mut term, mut s, mut neg) = (E18, E18, false);
    for i in 1..256u64 {
        let k = mu(U256::from(i), E18)?;
        let mut c = su(k, E18)?;
        if otherpow > c {
            c = otherpow - c;
            neg = !neg;
        } else {
            c -= otherpow;
        }
        term = du(mu(term, du(mu(c, x)?, E18)?)?, k)?;
        s = if neg { su(s, term)? } else { au(s, term)? };
        if term < precision {
            return du(mu(result, s)?, E18);
        }
    }
    None
}

/// `sqrt_int(x)` (L293-L310): корень в единицах 10^18 (z = (x·10^18/z + z)/2 до z = y). None — откат. Ошибок нет.
pub fn sqrt_int(x: U256) -> Option<U256> {
    if x.is_zero() {
        return Some(U256::ZERO);
    }
    let two = U256::from(2u8);
    let mut z = au(x, E18)? / two;
    let mut y = x;
    for _ in 0..256 {
        if z == y {
            return Some(y);
        }
        y = z;
        z = au(du(mu(x, E18)?, z)?, z)? / two;
    }
    None
}

/// Оракул на время `now` (tweak_price: при last_prices_timestamp < now — скользящее среднее, иначе как есть).
/// Принимает: последнюю цену, сохранённый оракул, last_prices_timestamp, ma_half_time, время. Возвращает: оракул или
/// None (откат: деление на ноль ma_half_time, halfpow). Ошибок нет.
pub fn oracle_at(last: U256, oracle: U256, lpt: U256, mht: U256, now: U256) -> Option<U256> {
    if lpt >= now {
        return Some(oracle);
    }
    let alpha = halfpow(du(mu(now - lpt, E18)?, mht)?, E10)?;
    du(au(mu(last, su(E18, alpha)?)?, mu(oracle, alpha)?)?, E18)
}
