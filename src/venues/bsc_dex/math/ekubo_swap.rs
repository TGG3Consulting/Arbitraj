//! Файл: venues/bsc_dex/math/ekubo_swap.rs
//! Что делает: обмен в пуле Ekubo на точный вход (решение 447) — точный порт цикла swap ядра (Core.swap_6269342730,
//! ветка exact-in) по прочитанному состоянию: шаг до следующего тика битмапа (skipAhead = 0) или границы диапазона
//! стейблсвапа, комиссия со входа, пересечение инициализированного тика меняет ликвидность, предел цены —
//! MIN / MAX_SQRT_RATIO (как шаг EKUBO исполнителя и квотер сверки). Нужное слово битмапа не прочитано — расчёт
//! останавливается: complete = false (сколько входа принято до края карты). Доплата MEVCapture — math/ekubo_mev.rs.
//! Кто вызывает: quote/pool_quote.rs (ветка Ekubo), quote/depth_band.rs.
//! От чего зависит: core/types (EkuboData), math/ekubo_sqrt.rs, ekubo_delta.rs, ekubo_tick.rs, ekubo_bitmap.rs,
//! ekubo_config.rs.
//! Настройки: нет.
//!
//! Источник — EkuboProtocol/evm-contracts@61d09daa src/Core.sol L510-L858 (swap): L545 (нулевая сумма / цена = предел —
//! без обмена), L550-L558 (направление: increasing = isToken1 xor isExactOut), L576-L620 (следующий тик: стейблсвап /
//! битмап), L622-L623 (предел шага), L627-L629 (пустой шаг — до предела), L638-L652 (комиссия со входа и цена по сумме),
//! L666-L701 (упёрлись в предел шага: суммы по границе, вход с комиссией amountBeforeFee), L702-L727 (сумма кончилась
//! внутри шага), L728-L738 (цена не сдвинулась — весь остаток в комиссию), L756-L808 (пересечение тика, ликвидность ±
//! liquidityDelta; иначе тик по цене), L810-L812 (выход из цикла), L815-L826 (итог). Только exact-in: исполнитель
//! меняет точный вход.

use alloy_primitives::U256;

use crate::core::types::EkuboData;

use super::ekubo_bitmap::{self, delta_at};
use super::ekubo_config::{self, Shape};
use super::ekubo_delta::{amount0, amount0_sorted, amount1, amount1_sorted, sorted};
use super::ekubo_sqrt::{
    MAX_SQRT_RATIO, MIN_SQRT_RATIO, amount_before_fee, compute_fee, next_from_amount0,
    next_from_amount1,
};
use super::ekubo_tick::{MAX_TICK, MIN_TICK, tick_at, to_sqrt_ratio_at};

/// Итог обмена.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Out {
    /// Принято входа (сумма минус остаток).
    pub amount_in: u128,
    /// Отдано выхода (до доплаты MEVCapture).
    pub amount_out: u128,
    /// Весь вход принят (расчёт не упёрся в непрочитанное слово битмапа).
    pub complete: bool,
    /// Тик пула после обмена (для доплаты MEVCapture).
    pub tick_after: i32,
    /// Цена после обмена равна пределу (обмен дошёл до предела цены).
    pub at_limit: bool,
}

/// Обмен на точный вход до предела цены MIN / MAX_SQRT_RATIO (как шаг EKUBO и квотер сверки). Принимает: состояние,
/// вход — token1 (иначе token0), сумму (> 0, ≤ int128). Возвращает: итог. None — обмен откатился бы (переполнение
/// сумм, недопустимая цена) или сумма не помещается в int128.
pub fn exact_in(d: &EkuboData, is_token1: bool, amount: u128) -> Option<Out> {
    let limit = U256::from(if is_token1 {
        MAX_SQRT_RATIO
    } else {
        MIN_SQRT_RATIO
    });
    exact_in_to(d, is_token1, amount, limit)
}

/// Обмен на точный вход до заданного предела цены (сжатая цена; глубина в полосе — quote/depth_ekubo.rs). Принимает:
/// состояние, вход — token1, сумму, предел. Возвращает: итог. None — как у exact_in, и предел не в сторону обмена
/// (SqrtRatioLimitWrongDirection, Core.sol L556-L558).
pub fn exact_in_to(d: &EkuboData, is_token1: bool, amount: u128, limit: U256) -> Option<Out> {
    let specified = i128::try_from(amount).ok()?;
    let fee = ekubo_config::fee(&d.config);
    let shape = ekubo_config::shape(&d.config);
    let increasing = is_token1;
    if limit != d.sqrt_ratio && (limit < d.sqrt_ratio) == increasing {
        return None;
    }
    let (mut sqrt, mut tick, mut liquidity) = (d.sqrt_ratio, d.tick, d.liquidity);
    let mut remaining = specified;
    let mut calculated: u128 = 0;
    let mut complete = true;
    if remaining == 0 || sqrt == limit {
        return Some(Out {
            amount_in: 0,
            amount_out: 0,
            complete,
            tick_after: tick,
            at_limit: sqrt == limit,
        });
    }
    loop {
        let (next_tick, initialized, step_liquidity) = match next_step(d, shape, tick, increasing) {
            Some((t, i, active)) => (t, i, if active { liquidity } else { 0 }),
            None => {
                complete = false;
                break;
            }
        };
        let next_sqrt = to_sqrt_ratio_at(next_tick)?;
        let limited = if increasing {
            next_sqrt.min(limit)
        } else {
            next_sqrt.max(limit)
        };
        let sqrt_next = if step_liquidity == 0 {
            limited
        } else {
            let fee_amount = compute_fee(remaining as u128, fee) as i128;
            let impact = remaining - fee_amount;
            let from_amount = if is_token1 {
                next_from_amount1(sqrt, step_liquidity, impact)?
            } else {
                next_from_amount0(sqrt, step_liquidity, impact)?
            };
            let hit = if increasing {
                from_amount > limited
            } else {
                from_amount < limited
            };
            if hit {
                let (lo, hi) = sorted(limited, sqrt);
                let (spec, calc) = if is_token1 {
                    let s = amount1_sorted(lo, hi, step_liquidity, true)?;
                    (s, amount0_sorted(lo, hi, step_liquidity, false)?)
                } else {
                    let s = amount0_sorted(lo, hi, step_liquidity, true)?;
                    (s, amount1_sorted(lo, hi, step_liquidity, false)?)
                };
                let before = amount_before_fee(spec, fee)?;
                calculated = calculated.checked_add(calc)?;
                remaining = remaining.checked_sub(i128::try_from(before).ok()?)?;
                limited
            } else if from_amount != sqrt {
                let calc = if is_token1 {
                    amount0(from_amount, sqrt, step_liquidity, false)?
                } else {
                    amount1(from_amount, sqrt, step_liquidity, false)?
                };
                calculated = calculated.checked_add(calc)?;
                remaining = 0;
                from_amount
            } else {
                // Цена не сдвинулась — весь остаток уходит в комиссию (Core.sol L728-L738).
                remaining = 0;
                sqrt
            }
        };
        if sqrt_next == next_sqrt {
            sqrt = sqrt_next;
            tick = next_tick - i32::from(!increasing);
            if initialized {
                let delta = delta_at(d, next_tick);
                let signed = if increasing {
                    delta
                } else {
                    delta.checked_neg()?
                };
                liquidity = liquidity.checked_add_signed(signed)?;
            }
        } else if sqrt != sqrt_next {
            sqrt = sqrt_next;
            tick = tick_at(sqrt)?;
        }
        if remaining == 0 || sqrt == limit {
            break;
        }
    }
    Some(Out {
        amount_in: (specified - remaining) as u128,
        amount_out: calculated,
        complete,
        tick_after: tick,
        at_limit: sqrt == limit,
    })
}

/// Следующий тик шага: (тик, инициализирован, ликвидность шага активна). None — слово битмапа не прочитано.
/// Стейблсвап — Core.sol L576-L602, концентрированный — L604-L619 (skipAhead = 0). Ошибок нет.
fn next_step(
    d: &EkuboData,
    shape: Shape,
    tick: i32,
    increasing: bool,
) -> Option<(i32, bool, bool)> {
    match shape {
        Shape::FullRange => Some((if increasing { MAX_TICK } else { MIN_TICK }, false, true)),
        Shape::Stable { lower, upper } => {
            if (lower..upper).contains(&tick) {
                return Some((if increasing { upper } else { lower }, false, true));
            }
            if tick < lower {
                return Some((if increasing { lower } else { MIN_TICK }, false, false));
            }
            Some((if increasing { MAX_TICK } else { upper }, false, false))
        }
        Shape::Concentrated { spacing } => {
            let (t, i) = if increasing {
                ekubo_bitmap::next(d, tick, spacing)?
            } else {
                ekubo_bitmap::prev(d, tick, spacing)?
            };
            Some((t, i, true))
        }
    }
}
