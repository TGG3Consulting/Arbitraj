//! Файл: venues/bsc_dex/math/maverick_swap.rs
//! Что делает: обмен в пуле Maverick V2 по прочитанным тикам (решение 122 F) — цикл swap пула: тик за тиком от
//! активного (вход tokenA — тики вверх, tokenB — вниз), пустые тики пропускаются, предел тика, масштаб токенов.
//! Порт команды Maverick, Solidity-исходник закрыт (решение Тиграна 03.11): MaverickV2Pool.sol (swap, _swapTick) —
//! в приватном maverickprotocol/maverick-v2; портировано с TS-порта команды Maverick — VeloraDEX/paraswap-dex-lib
//! 4f74b83bb14d9b2c951a7cda673f0cca29dcad81, src/dex/maverick-v2/maverick-math/maverick-pool-math.ts: estimateSwap
//! L71-L137, combine L60-L69, swapTick L633-L698, tickSqrtPriceAndLiquidity L611-L631, allocateSwapValuesToTick
//! L704-L727; pastMaxTick — maverick-delta-math.ts L16-L26. Перенос бинов (moveBins) и TWAP идут ПОСЛЕ обмена и на его
//! суммы не влияют (swap L201-L216) — здесь не считаются. Цена и L тика — официальный TickMath (math/maverick_tick_math.rs).
//! Повторено как есть (аудит Omniscia, MVP-03M, «acknowledged»): если цикл проходит несколько пустых тиков подряд и
//! упирается в предел, активный тик откатывается только на ±1 от прежнего (L642-L651) — влияет на итоговый активный тик
//! (TWAP), не на суммы.
//! Своё (не из порта): тик вне прочитанного диапазона [tick_lo, tick_hi] неизвестен — обмен останавливается, итог
//! частичный (complete = false), как у V3 за прочитанной картой.
//! Сверка с MaverickV2Quoter.calculateSwap — перед каждой сделкой и в цикле (решение 73, quote/quoter_maverick.rs).
//! Кто вызывает: quote/pool_quote.rs, quote/depth_band.rs, quote/spot.rs; тесты (tests/bsc_dex_maverick_family.rs).
//! От чего зависит: math/maverick_swap_math.rs, math/maverick_tick_math.rs, math/maverick_math.rs, core/types.
//! Настройки: нет.

use std::collections::BTreeMap;

use alloy_primitives::U256;

use crate::core::types::TickBinData;

use super::maverick_math::{amm_scale_to_token_scale, clip};
use super::maverick_swap_math::{self as sm, Step, TickData};
use super::maverick_tick_math::{get_tick_sqrt_price_and_l, tick_sqrt_prices};

/// Итог обмена: суммы в единицах токенов, полон ли (вся сумма исполнена в прочитанных тиках, предел не достигнут),
/// активный тик после обмена (до переноса бинов).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Swapped {
    pub amount_in: U256,
    pub amount_out: U256,
    pub complete: bool,
    /// Остановились на непрочитанном тике (своё правило; иначе неполнота — упор в предел тика).
    pub unread: bool,
    pub end_tick: i32,
}

/// Что сделал один шаг цикла.
enum Flow {
    /// Шаг в тике (его итог прибавить).
    Step(Step),
    /// Достигнут предел тика (pastMaxTick): остаток обнуляется, суммы не прибавляются.
    Limit,
    /// Следующий тик не прочитан — остановка (своё правило, см. шапку).
    Unread,
}

/// Обмен (estimateSwap). Принимает: состояние, сумму (вход или выход, единицы токена), вход tokenA (= token0), точный
/// выход, предел тика (без предела: i32::MAX для входа tokenA, i32::MIN для tokenB — IMaverickV2Pool.sol L102-L104).
/// None — пул откатил бы обмен (тик за пределом, точный выход больше резерва, переполнение).
pub fn swap(
    s: &TickBinData,
    amount: U256,
    a_in: bool,
    exact_out: bool,
    tick_limit: i32,
) -> Option<Swapped> {
    let start = s.active_tick;
    if (start > tick_limit && a_in) || (start < tick_limit && !a_in) {
        return None; // L96-L101: Beyond Swap Limit (PoolCurrentTickBeyondSwapLimit)
    }
    let used = if a_in { s.reserve1 } else { s.reserve0 };
    if exact_out && amount > used {
        return None; // L103-L106
    }
    let (in_scale, out_scale) = if a_in {
        (s.scale0, s.scale1)
    } else {
        (s.scale1, s.scale0)
    };
    let scale = if exact_out { out_scale } else { in_scale };
    let mut excess = if scale == U256::from(1u8) {
        amount
    } else {
        amount.checked_mul(scale)?
    };
    let mut ticks: BTreeMap<i32, (U256, U256)> = s
        .ticks
        .iter()
        .map(|t| (t.tick, (U256::from(t.reserve0), U256::from(t.reserve1))))
        .collect();
    let (mut active, mut total, mut swapped_max, mut unread) =
        (start, Step::default(), false, false);
    while !excess.is_zero() {
        match swap_tick(
            s,
            &mut ticks,
            &mut active,
            excess,
            a_in,
            exact_out,
            tick_limit,
        )? {
            Flow::Step(d) => {
                total.in_bin_internal = total.in_bin_internal.checked_add(d.in_bin_internal)?;
                total.in_erc = total.in_erc.checked_add(d.in_erc)?;
                total.out_erc = total.out_erc.checked_add(d.out_erc)?;
                excess = d.excess;
                swapped_max = false;
            }
            Flow::Limit => {
                excess = U256::ZERO;
                swapped_max = true;
            }
            Flow::Unread => {
                unread = true;
                break;
            }
        }
    }
    Some(Swapped {
        // L124-L134: вход вверх при точном выходе или упоре в предел.
        amount_in: amm_scale_to_token_scale(
            total.in_erc,
            in_scale,
            exact_out || swapped_max || unread,
        ),
        amount_out: amm_scale_to_token_scale(total.out_erc, out_scale, false),
        complete: !swapped_max && !unread,
        unread,
        end_tick: active,
    })
}

/// pastMaxTick (maverick-delta-math.ts L17-L26): тик за пределом обмена. Ошибок нет.
fn past_max(a_in: bool, tick_limit: i32, tick: i32) -> bool {
    if a_in {
        tick_limit < tick
    } else {
        tick_limit > tick
    }
}

/// Один шаг (swapTick, L633-L698). Принимает: состояние, тики (меняются как у пула), активный тик (меняется),
/// остаток, направление, точный выход, предел. None — откат (переполнение, тик вне TickMath).
fn swap_tick(
    s: &TickBinData,
    ticks: &mut BTreeMap<i32, (U256, U256)>,
    active: &mut i32,
    excess: U256,
    a_in: bool,
    exact_out: bool,
    tick_limit: i32,
) -> Option<Flow> {
    let back = if a_in { -1 } else { 1 };
    let mut at = *active;
    if past_max(a_in, tick_limit, at) {
        *active = active.checked_add(back)?; // L637-L640
        return Some(Flow::Limit);
    }
    let empty = |t: &BTreeMap<i32, (U256, U256)>, k: i32| {
        t.get(&k).is_none_or(|(a, b)| a.is_zero() && b.is_zero())
    };
    while empty(ticks, at) {
        at = at.checked_add(-back)?; // L646
        if past_max(a_in, tick_limit, at) {
            *active = active.checked_add(back)?; // L648-L651: откат только на ±1 от прежнего активного (MVP-03M)
            return Some(Flow::Limit);
        }
        if at < s.tick_lo || at > s.tick_hi {
            return Some(Flow::Unread);
        }
    }
    let (ra, rb) = *ticks.get(&at)?;
    let (lower, upper) = tick_sqrt_prices(s.tick_spacing, at)?;
    let (sqrt_price, liquidity) = get_tick_sqrt_price_and_l(ra, rb, lower, upper)?;
    let t = TickData {
        reserve_a: ra,
        reserve_b: rb,
        liquidity,
    };
    *active = at; // L661
    let fee = if a_in { s.fee0_in } else { s.fee1_in };
    let d = if exact_out {
        sm::exact_out(sqrt_price, t, excess, a_in, fee, s.protocol_fee_d3)?
    } else {
        sm::exact_in(sqrt_price, t, excess, a_in, fee, s.protocol_fee_d3)?
    };
    // allocateSwapValuesToTick (L704-L727): вход остаётся в тике, выход уходит; тик выработан — его выход 0.
    let drained = !d.excess.is_zero();
    let slot = ticks.get_mut(&at)?;
    if a_in {
        slot.0 = slot.0.checked_add(d.in_bin_internal)?;
        slot.1 = if drained {
            U256::ZERO
        } else {
            clip(slot.1, d.out_erc)
        };
    } else {
        slot.0 = if drained {
            U256::ZERO
        } else {
            clip(slot.0, d.out_erc)
        };
        slot.1 = slot.1.checked_add(d.in_bin_internal)?;
    }
    if drained {
        *active = active.checked_sub(back)?; // L691-L695
    }
    Some(Flow::Step(d))
}
