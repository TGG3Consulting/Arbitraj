//! Файл: venues/bsc_dex/math/lb_swap.rs
//! Что делает: обмен в пуле Liquidity Book V2.1 (решение 290) на локальных резервах бинов — точный порт LBPair.getSwapOut
//! (тот же цикл, что у swap): опорные значения по времени блока, затем по бинам от активного — в бине с резервом выхода
//! накопитель волатильности по этому бину, полная комиссия и BinHelper.getAmounts; следующий непустой бин — swapForY
//! вниз по id, иначе вверх. Только точный вход (шаг исполнителя переводит вход и зовёт swap). Бин вне прочитанной полосы
//! — остановка, итог частичный (complete = false); бины пула кончились — тоже (swap пула откатился бы
//! LBPair__OutOfLiquidity, getSwapOut вернул бы остаток входа).
//! Решение 445: пул Liquidity Book v2.0 (LbBinData.v20) — тот же цикл (LBPair.sol v2.0.0 swap L358-L392: бин с резервом
//! выхода, накопитель по бину, getAmounts; следующий непустой бин — findFirstBin, swapForY — вниз), один бин — по
//! правилам v2.0 (math/lb20_swap.rs), опорные значения и накопитель — lb_fee.rs (ветка v20).
//! Кто вызывает: quote/pool_quote.rs, quote/depth_band.rs, tests/bsc_dex_lb_v21_live.rs.
//! От чего зависит: math/lb_fee.rs, math/bin_price.rs (getPriceFromId — те же строки, что у v2.1.0: Uint128x128Math.pow
//! сверен построчно), math/full_math.rs (mulShift / shiftDiv — сверены с Uint256x256Math v2.1.0), pools/bins.rs.
//! Настройки: нет.
//!
//! Источник (решение 23): lfj-gg/joe-v2@v2.1.0 — src/LBPair.sol getSwapOut L424-L468, swap L483-L557 (L538 —
//! OutOfLiquidity, L544 — InsufficientAmountOut), _getNextNonEmptyBin L842-L844 (swapForY → findFirstRight: строго ниже;
//! иначе findFirstLeft: строго выше); src/libraries/BinHelper.sol getAmounts L222-L267; src/libraries/PriceHelper.sol
//! getPriceFromId L28-L33; src/libraries/Constants.sol L11 (SCALE_OFFSET = 128). Факт цепи: = getSwapOut пула в 50 из
//! 50 ответов (5 пулов × 5 сумм × 2 стороны, блок 133716961 — tests/bsc_dex_lb_v21_live.rs).

use alloy_primitives::U256;

use crate::core::types::LbBinData;

use super::super::pools::bins::BinMap;
use super::bin_price::{SCALE_OFFSET, price_from_id};
use super::full_math::{mul_shift, shift_div};
use super::lb_fee::{self, Volatility};

/// Итог обмена на точный вход — поля ответа getSwapOut (amountInLeft, amountOut, fee) и полнота.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LbOut {
    /// Сколько входа не исполнено.
    pub in_left: u128,
    /// Сколько получено.
    pub out: u128,
    /// Сумма комиссий по бинам (в токене входа).
    pub fee: u128,
    /// true — весь вход исполнен в прочитанной полосе бинов.
    pub complete: bool,
}

/// Обмен на точный вход. Принимает: состояние пула, резервы бинов (X, Y — токены пула X и Y), время блока обмена,
/// направление (true — X за Y), сумму входа. Возвращает: LbOut; None — там, где контракт откатился бы (комиссия больше
/// MAX_FEE, safe128, цена бина не считается, время блока раньше последнего обновления).
pub fn swap_out(
    d: &LbBinData,
    bins: &BinMap,
    now: u64,
    swap_for_y: bool,
    amount: u128,
) -> Option<LbOut> {
    let mut v: Volatility = lb_fee::references(d, now)?;
    let mut id = d.active_id;
    let (mut left, mut out, mut fee) = (amount, 0u128, 0u128);
    loop {
        if !bins.covers(id) {
            return Some(LbOut {
                in_left: left,
                out,
                fee,
                complete: false,
            });
        }
        let (rx, ry) = bins.reserves.get(&id).copied().unwrap_or((0, 0));
        let reserve_out = if swap_for_y { ry } else { rx };
        if reserve_out != 0 {
            v = lb_fee::accumulate(v, d, id)?;
            let total = lb_fee::total_fee(d, v.acc)?;
            let (used, got, f) = if d.v20 {
                super::lb20_swap::amounts(id, d.bin_step, (rx, ry), total, swap_for_y, left)?
            } else {
                amounts(id, d.bin_step, reserve_out, total, swap_for_y, left)?
            };
            left -= used;
            out = out.checked_add(got)?;
            fee = fee.checked_add(f)?;
        }
        if left == 0 {
            return Some(LbOut {
                in_left: 0,
                out,
                fee,
                complete: true,
            });
        }
        let next = if swap_for_y {
            bins.reserves.range(..id).next_back().map(|(k, _)| *k)
        } else {
            bins.reserves.range(id + 1..).next().map(|(k, _)| *k)
        };
        match next {
            Some(n) => id = n,
            // Непустых в полосе больше нет: полоса кончилась или бины пула исчерпаны (swap — OutOfLiquidity, L538).
            None => {
                return Some(LbOut {
                    in_left: left,
                    out,
                    fee,
                    complete: false,
                });
            }
        }
    }
}

/// BinHelper.getAmounts (L222-L267) в одном бине. Принимает: бин, шаг, резерв выхода, полную комиссию, направление,
/// остаток входа. Возвращает: (вход с комиссией, выход, комиссия); None — safe128 или цена не считается.
fn amounts(
    id: u32,
    step: u32,
    reserve_out: u128,
    total_fee: u128,
    swap_for_y: bool,
    in_left: u128,
) -> Option<(u128, u128, u128)> {
    let price = price_from_id(id, step)?;
    let max_in = if swap_for_y {
        shift_div(U256::from(reserve_out), SCALE_OFFSET, price, true)?
    } else {
        mul_shift(U256::from(reserve_out), price, SCALE_OFFSET, true)?
    };
    let max_in: u128 = max_in.try_into().ok()?;
    let max_fee = lb_fee::fee_on(max_in, total_fee)?;
    // maxAmountIn += maxFee (L241): сложение uint128 с проверкой переполнения (solc 0.8).
    let max_in = max_in.checked_add(max_fee)?;
    if in_left >= max_in {
        return Some((max_in, reserve_out, max_fee));
    }
    let f = lb_fee::fee_from(in_left, total_fee)?;
    let net = U256::from(in_left - f);
    let got = if swap_for_y {
        mul_shift(net, price, SCALE_OFFSET, false)?
    } else {
        shift_div(net, SCALE_OFFSET, price, false)?
    };
    let got: u128 = got.try_into().ok()?;
    Some((in_left, got.min(reserve_out), f))
}
