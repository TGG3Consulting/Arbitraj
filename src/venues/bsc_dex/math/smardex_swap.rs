//! Файл: venues/bsc_dex/math/smardex_swap.rs
//! Что делает: обмен пары SmarDex на точный вход так, как его исполнит пара (решение 288): средняя цена к времени блока
//! (порт SmardexLibrary.getUpdatedPriceAverage), затем getAmountOut (math/smardex.rs) и проверки _update пары. Вынесено из
//! smardex.rs (предел размера файла).
//! Кто вызывает: quote/pool_quote.rs (ветка Fictive), quote/depth_band.rs (side); тесты на записанных ответах цепи.
//! От чего зависит: math/smardex.rs, core/types (FictiveData), alloy-primitives.
//! Настройки: нет.
//!
//! Источник — SmardexLibrary.sol getUpdatedPriceAverage L76-L109 (MAX_BLOCK_DIFF_SECONDS = 300, L24); SmardexPair.sol swap
//! L155-L319: средняя цена к block.timestamp L202-L214 (SafeCast.toUint128), _update L350-L374 (фиктивные резервы не 0 и
//! не больше uint128 — L358-L359). Ссылки на репозиторий и Sourcify — шапка math/smardex.rs.

use alloy_primitives::U256;

use crate::core::types::FictiveData;

use super::smardex::{Params, amount_out};

/// MAX_BLOCK_DIFF_SECONDS = 300 (SmardexLibrary.sol L24) — за сколько секунд средняя цена догоняет фиктивные резервы.
const MAX_DIFF_S: u64 = 300;

/// Число u64 как U256 (константы протокола в формулах). Принимает: число. Возвращает: U256. Ошибок нет.
fn u(x: u64) -> U256 {
    U256::from(x)
}

/// Сторона пары по направлению: резервы, фиктивные резервы и средняя цена — вход / выход, комиссии пары. Принимает:
/// состояние пары, вход — token0. Ошибок нет.
pub fn side(d: &FictiveData, zero_for_one: bool) -> Params {
    let (reserve_in, reserve_out, fictive_in, fictive_out, price_in, price_out) = if zero_for_one {
        (
            d.reserve0,
            d.reserve1,
            d.fictive0,
            d.fictive1,
            d.price_average0,
            d.price_average1,
        )
    } else {
        (
            d.reserve1,
            d.reserve0,
            d.fictive1,
            d.fictive0,
            d.price_average1,
            d.price_average0,
        )
    };
    Params {
        reserve_in,
        reserve_out,
        fictive_in,
        fictive_out,
        price_in,
        price_out,
        fees_lp: d.fees_lp,
        fees_pool: d.fees_pool,
    }
}

/// getUpdatedPriceAverage (L76-L109). Принимает: фиктивные резервы (вход, выход), время последней средней цены, среднюю
/// цену (вход, выход), время блока. Возвращает: новую среднюю цену (вход, выход). None — время блока раньше записанного
/// (require) или переполнение / деление на 0.
pub fn price_average(
    f: (U256, U256),
    last_ts: u64,
    p: (U256, U256),
    now: u64,
) -> Option<(U256, U256)> {
    if now < last_ts {
        return None;
    }
    if last_ts == 0 {
        return Some(f);
    }
    if last_ts == now {
        return Some(p);
    }
    let diff = u((now - last_ts).min(MAX_DIFF_S));
    let keep = (u(MAX_DIFF_S) - diff).checked_mul(p.1)?.checked_mul(f.0)?;
    let out = keep.checked_div(p.0)?.checked_add(diff.checked_mul(f.1)?)? / u(MAX_DIFF_S);
    Some((f.0, out))
}

/// Выход обмена так, как его исполнит пара (SmardexPair.swap L155-L319): средняя цена к времени блока, getAmountOut, и
/// проверка _update — новые фиктивные резервы не 0 и не больше uint128. Принимает: состояние стороны со СТАРОЙ средней
/// ценой, время её записи, время блока, вход. Возвращает: выход. None — как откат пары.
pub fn swap_out(p: &Params, last_ts: u64, now: u64, amount: U256) -> Option<U256> {
    let (pi, po) = price_average(
        (p.fictive_in, p.fictive_out),
        last_ts,
        (p.price_in, p.price_out),
        now,
    )?;
    // SafeCast.toUint128 средней цены (SmardexPair.sol L212-L214).
    let max = U256::from(u128::MAX);
    if pi > max || po > max {
        return None;
    }
    let s = Params {
        price_in: pi,
        price_out: po,
        ..*p
    };
    let (out, f_in, f_out) = amount_out(&s, amount)?;
    if f_in.is_zero() || f_out.is_zero() || f_in > max || f_out > max {
        return None;
    }
    Some(out)
}
