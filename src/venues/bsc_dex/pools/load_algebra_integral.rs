//! Файл: venues/bsc_dex/pools/load_algebra_integral.rs
//! Что делает: решение 188 — чтение состояния пула Algebra Integral (THENA V3, DexSwap) на заданном блоке и сборка
//! PoolState::Algebra + TickMap для той же математики, что у Algebra V1.9 (math/swap_algebra.rs: шаг цены
//! PriceMovementMath и пересечение тика у Integral те же — SwapCalculation.sol: zeroToOne вычитает liquidityDelta,
//! currentTick = nextTick − 1, иначе прибавляет, currentTick = nextTick).
//! Отличия от V1.9 (load_algebra.rs):
//! (1) тики связаны СПИСКОМ, битмап не нужен: от nextTickGlobal вверх по полю nextTick и от prevTickGlobal вниз по
//!     полю prevTick — по dex.integral_tick_steps тиков в каждую сторону. Между соседями списка инициализированных
//!     тиков нет (TickTree/TickManagement), поэтому прочитанный отрезок [нижний, верхний] известен ПОЛНОСТЬЮ. Концы
//!     списка — TickMath.MIN_TICK / MAX_TICK: дошли до конца — сторона известна до края.
//! (2) комиссия: при флаге DYNAMIC_FEE в pluginConfig — fee() пула (getCurrentFee() плагина), иначе
//!     globalState.lastFee. Добавку плагина (pluginFee) пул отдаёт только в событии SwapFee — её учитывают события
//!     (events_algebra_integral.rs), расхождение ловит сверка квотером (виртуальный квотер, решение 186).
//! Окно карты тиков математика считает словами по 256 сырых тиков (swap_algebra::window_ticks), поэтому отрезок
//! переводится в слова, ЦЕЛИКОМ лежащие внутри него (окно не шире известного — безопасная сторона: край отрезка
//! внутри неполного слова не используется). Текущий тик вне окна — ошибка (тиков прочитано мало, расчёт невозможен).
//! Кто вызывает: pools/load.rs.
//! От чего зависит: ctx.rs, node/call.rs, pools/abi_algebra_integral.rs, load_cl.rs (i24), ticks.rs, math/tick_math.rs.
//! Настройки: dex.integral_tick_steps [bsc_dex] — сколько инициализированных тиков читать в каждую сторону.

use std::collections::BTreeMap;

use alloy_primitives::{Address, U256};

use crate::core::types::PoolState;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::math::tick_math::{MAX_TICK, MIN_TICK};
use super::super::node::call;
use super::abi_algebra_integral::{DYNAMIC_FEE, IAlgebraIntegralPool as P};
use super::load_cl::i24;
use super::ticks::{TickData, TickMap};

/// Состояние и карта тиков пула Integral на блоке. Принимает: окружение, адрес пула, блок-тег.
/// Ошибки: NotConfigured (dex.integral_tick_steps), узел (в т.ч. откат fee() плагина), ответ не по ABI, тик вне int24,
/// список тиков не сходится, текущий тик вне прочитанного окна.
pub async fn load(ctx: &Ctx, pool: Address, tag: &str) -> GatewayResult<(PoolState, TickMap)> {
    let steps = ctx.int("dex.integral_tick_steps")?;
    let gs = call::view(ctx, pool, &P::globalStateCall {}, tag).await?;
    let liquidity = call::view(ctx, pool, &P::liquidityCall {}, tag).await?;
    let fee = if gs.pluginConfig & DYNAMIC_FEE != 0 {
        call::view(ctx, pool, &P::feeCall {}, tag).await?
    } else {
        gs.lastFee
    };
    let tick = gs.tick.as_i32();
    let next = call::view(ctx, pool, &P::nextTickGlobalCall {}, tag)
        .await?
        .as_i32();
    let prev = call::view(ctx, pool, &P::prevTickGlobalCall {}, tag)
        .await?
        .as_i32();
    let mut map: BTreeMap<i32, TickData> = BTreeMap::new();
    let up = walk(ctx, pool, tag, next, true, steps, &mut map).await?;
    let down = walk(ctx, pool, tag, prev, false, steps, &mut map).await?;
    let words = window(down, up).ok_or_else(|| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("Algebra Integral {pool}: прочитанные тики [{down}; {up}] не дают ни одного полного слова — увеличьте dex.integral_tick_steps"),
        )
    })?;
    let (lo, hi) = (i32::from(words.0) * 256, i32::from(words.1) * 256 + 255);
    if tick < lo.max(MIN_TICK) || tick > hi.min(MAX_TICK) {
        return Err(ctx.err(
            ErrorCode::BadResponse,
            format!("Algebra Integral {pool}: текущий тик {tick} вне окна [{lo}; {hi}] — увеличьте dex.integral_tick_steps"),
        ));
    }
    let state = PoolState::Algebra {
        sqrt_price_x96: U256::from(gs.price),
        tick,
        liquidity,
        fee: u32::from(fee),
    };
    Ok((
        state,
        TickMap {
            spacing: 1,
            ticks: map.into(),
            words,
        },
    ))
}

/// Обход списка тиков от `start` в одну сторону (up — по nextTick, иначе по prevTick), не больше `steps` тиков или до
/// конца списка. Тики с liquidityTotal > 0 — в карту. Возвращает: последний прочитанный тик (MIN_TICK / MAX_TICK —
/// дошли до конца). Ошибки: узел, ответ не по ABI, тик вне int24, список не движется в свою сторону.
async fn walk(
    ctx: &Ctx,
    pool: Address,
    tag: &str,
    start: i32,
    up: bool,
    steps: u64,
    map: &mut BTreeMap<i32, TickData>,
) -> GatewayResult<i32> {
    let (mut t, mut last) = (start, start);
    for _ in 0..steps.max(1) {
        let i = call::view(ctx, pool, &P::ticksCall { tick: i24(ctx, t)? }, tag).await?;
        if i.liquidityTotal > U256::ZERO {
            let gross = u128::try_from(i.liquidityTotal).map_err(|_| {
                ctx.err(
                    ErrorCode::BadResponse,
                    format!("тик {t}: liquidityTotal больше uint128"),
                )
            })?;
            map.insert(
                t,
                TickData {
                    gross,
                    net: i.liquidityDelta,
                },
            );
        }
        last = t;
        if (up && t >= MAX_TICK) || (!up && t <= MIN_TICK) {
            return Ok(t);
        }
        let n = if up { i.nextTick } else { i.prevTick }.as_i32();
        if (up && n <= t) || (!up && n >= t) {
            return Err(ctx.err(
                ErrorCode::BadResponse,
                format!("Algebra Integral {pool}: список тиков не сходится ({t} → {n})"),
            ));
        }
        t = n;
    }
    // Шаги кончились: известен отрезок до последнего ПРОЧИТАННОГО тика (следующий за ним ещё не прочитан).
    Ok(last)
}

/// Слова (по 256 сырых тиков) целиком внутри известного отрезка [down; up]. Конец списка (MIN_TICK / MAX_TICK) —
/// сторона до края. Возвращает: (первое, последнее) слово или None — ни одного полного слова. Ошибок нет.
pub fn window(down: i32, up: i32) -> Option<(i16, i16)> {
    let lo = if down <= MIN_TICK {
        MIN_TICK.div_euclid(256)
    } else {
        // Первое слово, начало которого не левее down: ceil(down / 256).
        (down + 255).div_euclid(256)
    };
    let hi = if up >= MAX_TICK {
        MAX_TICK.div_euclid(256)
    } else {
        // Последнее слово, конец которого (w·256 + 255) не правее up.
        (up + 1).div_euclid(256) - 1
    };
    (lo <= hi).then_some((lo as i16, hi as i16))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_inside_known_segment() {
        assert_eq!(window(-300, 700), Some((-1, 1)));
        assert_eq!(window(0, 255), Some((0, 0)));
        assert_eq!(window(1, 255), None);
        assert_eq!(
            window(MIN_TICK, MAX_TICK),
            Some((
                (MIN_TICK.div_euclid(256)) as i16,
                (MAX_TICK.div_euclid(256)) as i16
            ))
        );
    }
}
