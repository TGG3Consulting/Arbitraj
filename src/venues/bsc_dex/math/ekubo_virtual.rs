//! Файл: venues/bsc_dex/math/ekubo_virtual.rs
//! Что делает: исполнение виртуальных ордеров TWAMM Ekubo до времени блока (решение 454) — точный порт цикла
//! TWAMM._executeVirtualOrdersFromWithinLock по прочитанному состоянию: отрезки между инициализированными временами,
//! суммы продажи по скоростям, при встречных ордерах — цена по computeNextSqrtRatio и обмен в ядре до неё, при
//! односторонних — обмен до MIN / MAX_SQRT_RATIO; на инициализированном времени скорости меняются на дельты TimeInfo.
//! Ядро зовёт это перед каждым нашим обменом в пуле с TWAMM (TWAMM.beforeSwap), поэтому котировка считает обмен по
//! состоянию ПОСЛЕ виртуальных ордеров. Награды и сохранённые балансы TWAMM на цену не влияют — не считаются.
//! Кто вызывает: quote/pool_quote.rs, quote/depth_band.rs (ветка Ekubo, пул с TWAMM).
//! От чего зависит: core/types (EkuboData), math/ekubo_twamm.rs, ekubo_time.rs, ekubo_swap.rs, ekubo_sqrt.rs,
//! ekubo_config.rs.
//! Настройки: нет.
//!
//! Источник — EkuboProtocol/evm-contracts@61d09daa src/extensions/TWAMM.sol L394-L600: L409 (realLast), L412
//! (в этом блоке уже исполнено — ничего), L428-L445 (поиск времени и суммы), L449-L493 (встречные — цена и обмен до
//! неё), L494-L521 (односторонние — обмен до предела), L545-L567 (время инициализировано — скорости ± дельты);
//! src/types/twammPoolState.sol L20-L24 (realLastVirtualOrderExecutionTime).

use alloy_primitives::U256;

use crate::core::types::EkuboData;

use super::ekubo_config;
use super::ekubo_sqrt::{MAX_SQRT_RATIO, MIN_SQRT_RATIO};
use super::ekubo_swap::exact_in_to;
use super::ekubo_time::search;
use super::ekubo_twamm::{add_sale_rate_delta, amount_from_sale_rate, next_sqrt_ratio};

/// realLastVirtualOrderExecutionTime: T − ((T mod 2^32 − last) mod 2^32). Ошибок нет.
pub fn real_last(now: u64, last: u32) -> u64 {
    now - u64::from((now as u32).wrapping_sub(last))
}

/// Состояние пула после виртуальных ордеров к времени `now`. Принимает: состояние, время блока обмена. Возвращает:
/// копию с ценой, тиком, ликвидностью и состоянием TWAMM на `now` (без TWAMM — копию как есть). None — расчёт
/// невозможен: время за прочитанным горизонтом (TimeInfo / слова не прочитаны) или откат в контракте. Ошибок нет.
pub fn advance(d: &EkuboData, now: u64) -> Option<EkuboData> {
    let mut s = d.clone();
    let Some(tw) = d.twamm.as_ref() else {
        return Some(s);
    };
    let real = real_last(now, tw.last);
    if real == now {
        return Some(s);
    }
    if now > tw.until {
        return None;
    }
    let fee = ekubo_config::fee(&d.config);
    let (mut r0, mut r1, mut time) = (tw.rate0, tw.rate1, real);
    while time != now {
        let (next, init) = search(tw, real, time, now)?;
        let elapsed = next - time;
        let a0 = amount_from_sale_rate(r0, elapsed, false);
        let a1 = amount_from_sale_rate(r1, elapsed, false);
        if !a0.is_zero() && !a1.is_zero() {
            let target = next_sqrt_ratio(s.sqrt_ratio, s.liquidity, r0, r1, elapsed, fee)?;
            if target > s.sqrt_ratio {
                swap(&mut s, true, a1, target)?;
            } else if target < s.sqrt_ratio {
                swap(&mut s, false, a0, target)?;
            }
        } else if !a0.is_zero() {
            swap(&mut s, false, a0, U256::from(MIN_SQRT_RATIO))?;
        } else if !a1.is_zero() {
            swap(&mut s, true, a1, U256::from(MAX_SQRT_RATIO))?;
        }
        if init {
            let i = tw.times.binary_search_by_key(&next, |x| x.0).ok()?;
            let (_, d0, d1) = tw.times[i];
            r0 = add_sale_rate_delta(r0, d0)?;
            r1 = add_sale_rate_delta(r1, d1)?;
        }
        time = next;
    }
    if let Some(t) = s.twamm.as_mut() {
        t.last = now as u32;
        t.rate0 = r0;
        t.rate1 = r1;
    }
    Some(s)
}

/// Обмен ядра на точный вход до предела (CORE.swap из TWAMM, skipAhead 0) — цена, тик, ликвидность записи. None — откат
/// (сумма не int128, предел не в сторону обмена, расчёт за прочитанной картой). Ошибок нет.
fn swap(s: &mut EkuboData, is_token1: bool, amount: U256, limit: U256) -> Option<()> {
    let amount: u128 = amount.try_into().ok()?;
    let r = exact_in_to(s, is_token1, amount, limit)?;
    if !r.complete {
        return None;
    }
    s.sqrt_ratio = r.sqrt_after;
    s.tick = r.tick_after;
    s.liquidity = r.liquidity_after;
    Some(())
}
