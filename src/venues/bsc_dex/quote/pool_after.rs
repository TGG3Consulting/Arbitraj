//! Файл: venues/bsc_dex/quote/pool_after.rs
//! Что делает: состояние пула ПОСЛЕ обмена доли маршрута (решение 223) — чтобы следующая доля через тот же пул считалась
//! на том состоянии, на котором её исполнит сеть (исполнитель ArbExecutor и бандл проводят доли по очереди, каждая —
//! отдельным обменом в пуле). Обмен — точный вход (так исполняет и исполнитель: сумма шага = баланс × доля, и бандл:
//! вход шага — сумма кошелька или весь баланс роутера), без предела цены.
//! Виды пулов, для которых состояние после обмена считается точно, как в контракте:
//! V2 (PoolKind::V2, x·y=k): swap пары записывает резервы = балансы (_update(balance0, balance1)): вход пришёл в пару
//!   целиком (комиссия остаётся в паре), выход ушёл — reserveIn + amountIn, reserveOut − amountOut
//!   (UniswapV2Pair.sol swap — https://github.com/Uniswap/v2-core/blob/6a9e7c97860676e0992f22a49665760444c1cdf5/contracts/UniswapV2Pair.sol#L159-L187 ,
//!   _update — там же #L73-L86; PancakePair — тот же код). Протокольная комиссия V2 чеканится только в mint/burn (_mintFee), не в swap.
//! V3 (PoolKind::V3, Pancake/Uniswap V3): цена, тик и ликвидность после цикла swap (math/swap_cl.rs swap_final;
//!   запись slot0/liquidity — PancakeV3Pool.sol#L752-L781 по ссылке в шапке math/swap_cl.rs);
//!   карта тиков (liquidityNet) обменом не меняется, протокольная комиссия и рост комиссий ликвидность не трогают.
//! Остальные виды (V4/Infinity CL и Bin — крючок может менять состояние и комиссию между обменами; Elastic — reinvestL
//! растёт от комиссии; Algebra — динамическая комиссия плагина; Solidly — часть комиссии уходит из пары; StableSwap, DODO,
//! WooFi, Maverick) — состояние после обмена здесь НЕ считается: вторая доля через такой пул не допускается (честно —
//! без неверной цифры), см. quote/graph_seq.rs.
//! Кто вызывает: quote/graph_seq.rs.
//! От чего зависит: quote/pool_quote.rs (выход V2 — та же математика и комиссия, что у котировки), math/swap_cl.rs
//! (swap_final), pools/registry.rs (Entry), core/types (PoolKind, PoolState).
//! Настройки: через pool_quote — dex.v2_fee_pct [вид пула V2].

use alloy_primitives::{Address, U256};

use crate::core::types::{PoolKind, PoolState};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::math::swap_cl::{self, ClState, Swapped, Version};
use super::super::pools::registry::Entry;
use super::pool_quote;

/// Считается ли состояние пула после обмена (V2 и V3, см. шапку). Ошибок нет.
pub fn appliable(e: &Entry) -> bool {
    match (&e.pool.state, e.pool.kind) {
        (PoolState::V2 { .. }, PoolKind::V2) => true,
        (PoolState::V3 { .. }, PoolKind::V3) => e.ticks.is_some() && e.pool.fee_pips.is_some(),
        _ => false,
    }
}

/// Обмен точного входа в пуле и состояние пула после него. Принимает: окружение, пул (его текущее состояние — уже после
/// прошлых долей), токен входа, сумму входа (минимальные единицы). Возвращает: обмен и новое состояние пула.
/// Ошибки: NoRoute — вид пула без расчёта состояния после обмена, обмен не исполним целиком, переполнение резерва;
/// NotConfigured — комиссия V2 (dex.v2_fee_pct).
pub fn apply(
    ctx: &Ctx,
    e: &Entry,
    token_in: Address,
    amount: U256,
) -> GatewayResult<(Swapped, PoolState)> {
    let no = |w: &str| {
        ctx.err(
            ErrorCode::NoRoute,
            format!("{} {}: {w}", e.family.title, e.ticker),
        )
    };
    let zero_for_one = token_in == e.pool.token0;
    match (&e.pool.state, e.pool.kind) {
        (PoolState::V2 { reserve0, reserve1 }, PoolKind::V2) => {
            // Выход — та же формула и комиссия, что у котировки (pool_quote, getAmountOut).
            let s = pool_quote::swap(ctx, e, token_in, true, amount)?;
            let (in_r, out_r) = if zero_for_one {
                (*reserve0, *reserve1)
            } else {
                (*reserve1, *reserve0)
            };
            let new_in = in_r
                .checked_add(s.amount_in)
                .ok_or_else(|| no("резерв входа после обмена не помещается"))?;
            let new_out = out_r
                .checked_sub(s.amount_out)
                .ok_or_else(|| no("выход больше резерва"))?;
            let state = if zero_for_one {
                PoolState::V2 {
                    reserve0: new_in,
                    reserve1: new_out,
                }
            } else {
                PoolState::V2 {
                    reserve0: new_out,
                    reserve1: new_in,
                }
            };
            Ok((s, state))
        }
        (
            PoolState::V3 {
                sqrt_price_x96,
                liquidity,
                tick,
            },
            PoolKind::V3,
        ) => {
            let fee = e.pool.fee_pips.ok_or_else(|| no("нет комиссии пула V3"))?;
            let ticks = e.ticks.as_ref().ok_or_else(|| no("нет карты тиков"))?;
            let s = ClState {
                sqrt_price: *sqrt_price_x96,
                tick: *tick,
                liquidity: *liquidity,
                fee,
                ticks,
                version: Version::V3,
            };
            let (r, after) = swap_cl::swap_final(&s, zero_for_one, true, amount)
                .map_err(|w| no(&format!("расчёт обмена невозможен: {w}")))?;
            if !r.complete {
                return Err(no("обмен доли не исполним целиком в прочитанных тиках"));
            }
            let state = PoolState::V3 {
                sqrt_price_x96: after.sqrt_price,
                liquidity: after.liquidity,
                tick: after.tick,
            };
            Ok((r, state))
        }
        _ => Err(no(
            "состояние пула после обмена не считается для этого вида — вторая доля через пул не допускается",
        )),
    }
}
