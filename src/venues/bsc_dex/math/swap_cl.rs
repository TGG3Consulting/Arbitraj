//! Файл: venues/bsc_dex/math/swap_cl.rs
//! Что делает: обмен в пуле с концентрированной ликвидностью на локальной карте тиков — цикл swap контракта:
//! следующий инициализированный тик в пределах слова битмапа → шаг → пересечение тика (liquidityNet).
//! Две версии шага: V3 (PancakeSwap/Uniswap V3) и V4 (Infinity CL, Uniswap V4). Точный вход и точный выход.
//! Если цена уходит в слово битмапа, которое не прочитано, — остановка: результат частичный (complete = false).
//! swap_final — то же с состоянием пула после обмена (решение 223: следующая доля через тот же пул).
//! Кто вызывает: quote/direct.rs (через quote/pool_quote.rs), quote/pool_after.rs (swap_final).
//! От чего зависит: math/swap_step.rs, math/tick_math.rs, pools/ticks.rs.
//! Настройки: нет.
//!
//! Цикл: V3 — https://github.com/pancakeswap/pancake-v3-contracts/blob/986847948755cba528324d41be19480731c36c2a/projects/v3-core/contracts/PancakeV3Pool.sol#L663-L756 ;
//! V4 — https://github.com/Uniswap/v4-core/blob/bea7f6d1c11cd5dc4f0d5151ffa37a1c239feee6/src/libraries/Pool.sol#L339-L436 ;
//! nextInitializedTickWithinOneWord — v3-core contracts/libraries/TickBitmap.sol#L42-L77 (по карте тиков: все
//! инициализированные тики прочитанных слов известны). Предел цены, как у квотеров при limit = 0:
//! MIN_SQRT_RATIO + 1 / MAX_SQRT_RATIO − 1 (v3-periphery contracts/lens/QuoterV2.sol, quoteExactInputSingle).

use alloy_primitives::{I256, U256};

use super::super::pools::ticks::TickMap;
use super::swap_step::{step_v3, step_v4};
use super::tick_math::{
    MAX_TICK, MIN_TICK, max_sqrt_ratio, min_sqrt_ratio, sqrt_ratio_at_tick, tick_at_sqrt_ratio,
};

/// Версия математики шага.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Version {
    V3,
    V4,
}

/// Состояние пула для расчёта.
pub struct ClState<'a> {
    pub sqrt_price: U256,
    pub tick: i32,
    pub liquidity: u128,
    /// Полная комиссия обмена в пипсах (V3 — fee пула; V4 — protocol + LP по ProtocolFeeLibrary.calculateSwapFee).
    pub fee: u32,
    pub ticks: &'a TickMap,
    pub version: Version,
}

/// Итог обмена.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Swapped {
    /// Сколько отдаём в пул (с комиссией).
    pub amount_in: U256,
    /// Сколько получаем из пула.
    pub amount_out: U256,
    /// true — вся заданная сумма исполнена в прочитанных пределах карты тиков.
    pub complete: bool,
}

/// Обмен. Принимает: состояние, направление (true — токен0 → токен1), точный вход или выход, сумму.
/// Возвращает: Swapped; Err(причина) — там, где контракт откатился бы или карта тиков не сходится (конкретный шаг).
pub fn swap(
    s: &ClState,
    zero_for_one: bool,
    exact_in: bool,
    amount: U256,
) -> Result<Swapped, &'static str> {
    let limit = if zero_for_one {
        min_sqrt_ratio() + U256::from(1u8)
    } else {
        max_sqrt_ratio() - U256::from(1u8)
    };
    swap_to(s, zero_for_one, exact_in, amount, limit).map(|(r, _)| r)
}

/// Обмен до предела цены `limit` (sqrtPriceX96), как swap контракта с sqrtPriceLimitX96: цена дальше предела не идёт,
/// остаток суммы не исполняется (complete = false, если не дошли до предела и карта тиков кончилась). Решение 73:
/// глубина пула в полосе цены (quote/depth_band.rs). Принимает: состояние, направление, точный вход или выход,
/// сумму, предел. Возвращает: итог и дошла ли цена до предела (false — сумма кончилась раньше или кончились
/// прочитанные слова битмапа). Ошибки: как swap.
pub fn swap_to(
    s: &ClState,
    zero_for_one: bool,
    exact_in: bool,
    amount: U256,
    limit: U256,
) -> Result<(Swapped, bool), &'static str> {
    walk(s, zero_for_one, exact_in, amount, limit).map(|(r, at_limit, _)| (r, at_limit))
}

/// Состояние пула после обмена (решение 223): цена, тик, активная ликвидность — как slot0/liquidity контракта после swap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct After {
    pub sqrt_price: U256,
    pub tick: i32,
    pub liquidity: u128,
}

/// Обмен без предела цены (как swap) с состоянием пула ПОСЛЕ него — для следующей доли через тот же пул (решение 223,
/// quote/pool_after.rs). Тик — как в цикле контракта (PancakeV3Pool.sol#L752-L756 того же коммита, что в шапке; v4
/// Pool.sol — та же ветка): дошли до тика — tickNext − 1 / tickNext (это делает walk); последний шаг остановился внутри
/// диапазона и цена сдвинулась — getTickAtSqrtRatio(цена); slot0 и liquidity пишутся из state — #L760-L781. Принимает и ошибки: как swap; Err ещё — цена вне диапазона TickMath.
pub fn swap_final(
    s: &ClState,
    zero_for_one: bool,
    exact_in: bool,
    amount: U256,
) -> Result<(Swapped, After), &'static str> {
    let limit = if zero_for_one {
        min_sqrt_ratio() + U256::from(1u8)
    } else {
        max_sqrt_ratio() - U256::from(1u8)
    };
    let (r, _, mut after) = walk(s, zero_for_one, exact_in, amount, limit)?;
    if after.tick == i32::MIN {
        after.tick = tick_at_sqrt_ratio(after.sqrt_price)
            .ok_or("цена после обмена вне диапазона TickMath")?;
    }
    Ok((r, after))
}

/// Цикл обмена (общий для swap_to и swap_final). Возвращает: итог, дошла ли цена до предела, состояние после; у
/// состояния tick = i32::MIN — последний шаг остановился внутри диапазона со сдвигом цены и тик надо пересчитать по цене
/// (getTickAtSqrtRatio — только в swap_final, горячий расчёт котировки его не платит). Ошибки: как swap.
fn walk(
    s: &ClState,
    zero_for_one: bool,
    exact_in: bool,
    amount: U256,
    limit: U256,
) -> Result<(Swapped, bool, After), &'static str> {
    let (mut sqrt, mut tick, mut liq) = (s.sqrt_price, s.tick, s.liquidity);
    let (mut remaining, mut total_in, mut total_out) = (amount, U256::ZERO, U256::ZERO);
    const OVERFLOW: &str = "переполнение суммы";
    while !remaining.is_zero() && sqrt != limit {
        let Some((mut next, initialized)) = next_tick(s.ticks, tick, zero_for_one) else {
            let after = After {
                sqrt_price: sqrt,
                tick,
                liquidity: liq,
            };
            return Ok((
                Swapped {
                    amount_in: total_in,
                    amount_out: total_out,
                    complete: false,
                },
                false,
                after,
            ));
        };
        next = next.clamp(MIN_TICK, MAX_TICK);
        let sqrt_next = sqrt_ratio_at_tick(next).ok_or("цена тика вне диапазона TickMath")?;
        let target = if (zero_for_one && sqrt_next < limit) || (!zero_for_one && sqrt_next > limit)
        {
            limit
        } else {
            sqrt_next
        };
        let signed = I256::try_from(remaining).map_err(|_| OVERFLOW)?;
        let step = match (s.version, exact_in) {
            (Version::V3, true) => step_v3(sqrt, target, liq, signed, s.fee),
            (Version::V3, false) => step_v3(sqrt, target, liq, -signed, s.fee),
            (Version::V4, true) => step_v4(sqrt, target, liq, -signed, s.fee),
            (Version::V4, false) => step_v4(sqrt, target, liq, signed, s.fee),
        };
        let (new_sqrt, a_in, a_out, fee) =
            step.ok_or("шаг обмена не считается (переполнение или нулевая ликвидность на шаге)")?;
        let start = sqrt;
        sqrt = new_sqrt;
        let spent = a_in.checked_add(fee).ok_or(OVERFLOW)?;
        remaining = remaining
            .checked_sub(if exact_in { spent } else { a_out })
            .ok_or(OVERFLOW)?;
        total_in = total_in.checked_add(spent).ok_or(OVERFLOW)?;
        total_out = total_out.checked_add(a_out).ok_or(OVERFLOW)?;
        if sqrt == sqrt_next {
            if initialized {
                // Инициализированный тик без данных в карте — карта не сходится с битмапом: расчёт невозможен.
                let net = s
                    .ticks
                    .ticks
                    .get(&next)
                    .ok_or("тик отмечен в битмапе, но его нет в карте тиков")?
                    .net;
                let net = if zero_for_one {
                    net.checked_neg().ok_or(OVERFLOW)?
                } else {
                    net
                };
                liq = if net >= 0 {
                    liq.checked_add(net.unsigned_abs())
                } else {
                    liq.checked_sub(net.unsigned_abs())
                }
                .ok_or(
                    "ликвидность после пересечения тика вне диапазона (карта тиков не сходится)",
                )?;
            }
            tick = if zero_for_one { next - 1 } else { next };
        } else if sqrt != start && !remaining.is_zero() && sqrt != limit {
            // Шаг не дошёл до тика, но сумма не исчерпана — так в контракте не бывает; не гадаем.
            return Err("шаг остановился до тика при остатке суммы");
        } else if sqrt != start {
            // Контракт: state.tick = getTickAtSqrtRatio(цена); цикл на этом кончается — пересчёт в swap_final.
            tick = i32::MIN;
        }
    }
    let after = After {
        sqrt_price: sqrt,
        tick,
        liquidity: liq,
    };
    Ok((
        Swapped {
            amount_in: total_in,
            amount_out: total_out,
            complete: remaining.is_zero(),
        },
        sqrt == limit,
        after,
    ))
}

/// Следующий инициализированный тик в пределах слова (как TickBitmap.nextInitializedTickWithinOneWord).
/// Возвращает: (тик, инициализирован ли); None — нужное слово не прочитано.
pub fn next_tick(map: &TickMap, tick: i32, lte: bool) -> Option<(i32, bool)> {
    let sp = map.spacing;
    let compressed = tick.div_euclid(sp);
    let c = if lte { compressed } else { compressed + 1 };
    let word = (c >> 8) as i16;
    if word < map.words.0 || word > map.words.1 {
        return None;
    }
    let bit = c.rem_euclid(256);
    if lte {
        let start = (c - bit) * sp;
        match map.ticks.range(start..=c * sp).next_back() {
            Some((t, _)) => Some((*t, true)),
            None => Some((start, false)),
        }
    } else {
        let end = (c + (255 - bit)) * sp;
        match map.ticks.range(c * sp..=end).next() {
            Some((t, _)) => Some((*t, true)),
            None => Some((end, false)),
        }
    }
}
