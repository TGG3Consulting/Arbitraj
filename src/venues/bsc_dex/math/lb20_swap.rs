//! Файл: venues/bsc_dex/math/lb20_swap.rs
//! Что делает: решение 445 — обмен в одном бине пула Liquidity Book v2.0 (LFJ V2, Arbitrum) — точный порт
//! SwapHelper.getAmounts v2.0 и проверок пары при записи бина. Цикл по бинам, направление и опорные значения — общие с
//! v2.1 (math/lb_swap.rs swap_out; накопитель и опорные значения v2.0 — lb20_fee.rs через lb_fee.rs). Отличия от v2.1:
//!
//! - комиссия бина у v2.0 не входит в бин: в бин идёт вход без комиссии (amountInToBin), комиссия — отдельно
//!   (feesX / feesY пары); на выход это не влияет — выход и «сколько входа съедено» те же формулы;
//! - суммы — uint256 (у v2.1 — uint128 с откатом): предел наибольшего входа в бин нет, но вход без комиссии и выход
//!   проходят safe112 (LBPair.sol L366), резерв бина входа + вход — сложение uint112 с проверкой (SwapHelper.sol L115,
//!   L123, solc 0.8.10); комиссия — safe128 (FeeHelper.sol L154);
//! - предела MAX_FEE нет (lb20_fee.rs).
//!
//! Не моделируется (не меняет выход, откат только при переполнении uint128 накопленных комиссий пары или нулевом
//! totalSupply бина с резервом — SwapHelper.sol L88 (updateFees L81-L99), FeeDistributionHelper.sol L25): эти поля пары не читаются.
//! Кто вызывает: math/lb_swap.rs (amounts — ветка v20).
//! От чего зависит: math/bin_price.rs (getPriceFromId: Math128x128.power v2.0 = Uint128x128Math.pow v2.1 построчно,
//! основание BinHelper._getBPValue = PriceHelper.getBase), math/full_math.rs (mulShift / shiftDiv: Math512Bits v2.0 =
//! Uint256x256Math v2.1 по формулам), math/lb20_fee.rs.
//! Настройки: нет.
//!
//! Источник (решение 23): github.com/lfj-gg/joe-v2, тег v2.0.0 (07f10212e2de6abc97932240c27388c14868582c) —
//! src/libraries/SwapHelper.sol getAmounts L31-L73, updateReserves L107-L131; src/LBPair.sol swap L335-L429 (L360 —
//! бин с резервом выхода, L366 — safe112, L387-L388 — следующий непустой бин); src/libraries/BinHelper.sol
//! getPriceFromId L39-L46, _getBPValue L51-L58; src/libraries/Math128x128.sol power L94-L202;
//! src/libraries/Math512Bits.sol mulShiftRoundDown L56-L74, mulShiftRoundUp L92-L101, shiftDivRoundDown L119-L134,
//! shiftDivRoundUp L152-L161; src/libraries/SafeCast.sol safe112 L133-L135, safe128 L119-L121.

use alloy_primitives::U256;

use super::bin_price::{SCALE_OFFSET, price_from_id};
use super::full_math::{mul_shift, shift_div};
use super::lb20_fee;

/// Constants.BASIS_POINT_MAX = 10_000 (Constants.sol L13) — предел шага бина.
const BASIS_POINT_MAX: u32 = 10_000;
/// Наибольшее значение uint112 (safe112, резерв бина).
const MAX_U112: u128 = (1u128 << 112) - 1;

/// getAmounts (SwapHelper.sol L31-L73) в одном бине и проверки записи бина пары. Принимает: бин, шаг, резервы бина
/// (X, Y), полную комиссию (1e18), направление (true — X за Y), остаток входа. Возвращает: (съедено входа вместе с
/// комиссией, выход, комиссия); None — там, где пара откатилась бы (safe112 / safe128, переполнение резерва бина,
/// комиссия ≥ 1e18, цена бина не считается). Ошибок нет.
pub fn amounts(
    id: u32,
    step: u32,
    reserves: (u128, u128),
    total_fee: u128,
    swap_for_y: bool,
    in_left: u128,
) -> Option<(u128, u128, u128)> {
    // BinHelper._getBPValue (BinHelper.sol L51-L53): шаг 0 или больше 10000 — откат BinHelper__BinStepOverflows.
    if step == 0 || step > BASIS_POINT_MAX {
        return None;
    }
    let price = price_from_id(id, step)?;
    let (reserve_in, reserve_out) = if swap_for_y {
        (reserves.0, reserves.1)
    } else {
        (reserves.1, reserves.0)
    };
    // SwapHelper.sol L50-L56: наибольший вход в бин без комиссии — вверх.
    let max_in = if swap_for_y {
        shift_div(U256::from(reserve_out), SCALE_OFFSET, price, true)?
    } else {
        mul_shift(price, U256::from(reserve_out), SCALE_OFFSET, true)?
    };
    // SwapHelper.sol L58-L59: комиссия сверх наибольшего входа (getFeeAmount, safe128).
    let max_fee = lb20_fee::fee_on(max_in, total_fee)?;
    // SwapHelper.sol L61: вход с комиссией помещается в остаток — бин целиком.
    let (to_bin, got, fee) = if max_in.checked_add(max_fee)? <= U256::from(in_left) {
        let to_bin: u128 = max_in.try_into().ok()?;
        (to_bin, reserve_out, max_fee.to::<u128>())
    } else {
        // SwapHelper.sol L64-L72: комиссия из остатка (getFeeAmountFrom), вход без комиссии, выход вниз, не больше резерва.
        let f = lb20_fee::fee_from(in_left, total_fee)?;
        let to_bin = in_left.checked_sub(f)?;
        let net = U256::from(to_bin);
        let got = if swap_for_y {
            mul_shift(price, net, SCALE_OFFSET, false)?
        } else {
            shift_div(net, SCALE_OFFSET, price, false)?
        };
        let got: u128 = got.try_into().ok()?;
        (to_bin, got.min(reserve_out), f)
    };
    // LBPair.sol L366 (safe112 входа и выхода), SwapHelper.sol L115 / L123 (резерв бина входа += вход, uint112).
    if to_bin > MAX_U112 || got > MAX_U112 || reserve_in.checked_add(to_bin)? > MAX_U112 {
        return None;
    }
    Some((to_bin.checked_add(fee)?, got, fee))
}
