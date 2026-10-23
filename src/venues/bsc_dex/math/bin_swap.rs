//! Файл: venues/bsc_dex/math/bin_swap.rs
//! Что делает: обмен в пуле Infinity Bin на локальных резервах бинов — цикл BinPool.swap: в активном бине
//! getAmountsOut (точный вход) / getAmountsIn (точный выход), затем следующий непустой бин (swapForY — вниз
//! по id, иначе вверх). Бин вне прочитанной полосы — остановка, результат частичный.
//! Комиссии — FeeHelper (feeBips·1e12 при точности 1e18, округление вверх).
//! Кто вызывает: quote/pool_quote.rs.
//! От чего зависит: math/bin_price.rs, math/full_math.rs, pools/bins.rs, math/swap_cl.rs (Swapped).
//! Настройки: нет.
//!
//! Источники (infinity-core 3c5ef1be): BinPool.swap — src/pool-bin/libraries/BinPool.sol#L111-L221;
//! BinHelper.getAmountsOut — src/pool-bin/libraries/BinHelper.sol#L266-L311; getAmountsIn — #L230-L253;
//! FeeHelper — src/pool-bin/libraries/FeeHelper.sol#L13-L32. Протокольная часть комиссии остаётся в бине или
//! уходит протоколу — на суммы трейдера не влияет (BinPool.sol#L165-L172).

use alloy_primitives::U256;

use super::super::pools::bins::BinMap;
use super::bin_price::{SCALE_OFFSET, price_from_id};
use super::full_math::{mul_shift, shift_div};
use super::swap_cl::Swapped;

/// PRECISION = 1e18.
const PRECISION: u128 = 1_000_000_000_000_000_000;
/// Множитель feeBips → доля при PRECISION (feeBips·1e12).
const FEE_SCALE: u128 = 1_000_000_000_000;

/// FeeHelper.getFeeAmountFrom: комиссия из суммы с комиссией (вверх).
fn fee_from(amount_with_fees: u128, fee: u32) -> u128 {
    let total = U256::from(fee) * U256::from(FEE_SCALE);
    let r =
        (U256::from(amount_with_fees) * total + U256::from(PRECISION - 1)) / U256::from(PRECISION);
    r.to::<u128>()
}

/// FeeHelper.getFeeAmount: комиссия сверх суммы (вверх).
fn fee_on(amount: u128, fee: u32) -> u128 {
    let total = U256::from(fee) * U256::from(FEE_SCALE);
    let den = U256::from(PRECISION) - total;
    ((U256::from(amount) * total + den - U256::from(1u8)) / den).to::<u128>()
}

/// Обмен. Принимает: резервы бинов, активный бин, полную комиссию (пипсы: protocol + LP),
/// направление (true — X за Y), точный вход или выход, сумму.
/// Возвращает: Swapped; None — откат контракта (safe128, цена не считается).
pub fn swap(
    bins: &BinMap,
    active: u32,
    fee: u32,
    swap_for_y: bool,
    exact_in: bool,
    amount: u128,
) -> Option<Swapped> {
    let mut id = active;
    let (mut left, mut unspecified) = (amount, 0u128);
    loop {
        if !bins.covers(id) {
            return Some(done(amount, left, unspecified, exact_in, false));
        }
        let (rx, ry) = bins.reserves.get(&id).copied().unwrap_or((0, 0));
        let reserve_out = if swap_for_y { ry } else { rx };
        if reserve_out != 0 {
            let price = price_from_id(id, bins.step)?;
            let (in_with_fees, out_of_bin) = if exact_in {
                amounts_out(price, reserve_out, fee, swap_for_y, left)?
            } else {
                amounts_in(price, reserve_out, fee, swap_for_y, left)?
            };
            if exact_in {
                left -= in_with_fees;
                unspecified = unspecified.checked_add(out_of_bin)?;
            } else {
                left -= out_of_bin;
                unspecified = unspecified.checked_add(in_with_fees)?;
            }
        }
        if left == 0 {
            return Some(done(amount, 0, unspecified, exact_in, true));
        }
        // Следующий непустой бин: swapForY — строго ниже (findFirstRight), иначе строго выше (findFirstLeft).
        let next = if swap_for_y {
            bins.reserves.range(..id).next_back().map(|(k, _)| *k)
        } else {
            bins.reserves.range(id + 1..).next().map(|(k, _)| *k)
        };
        match next {
            Some(n) => id = n,
            // Непустых в полосе больше нет: либо полоса кончилась, либо ликвидность пула исчерпана
            // (контракт: BinPool__OutOfLiquidity). В обоих случаях исполнимо только то, что посчитано.
            None => return Some(done(amount, left, unspecified, exact_in, false)),
        }
    }
}

/// getAmountsOut в одном бине. Возвращает: (вход с комиссией, выход). None — safe128.
fn amounts_out(
    price: U256,
    reserve_out: u128,
    fee: u32,
    swap_for_y: bool,
    in_left: u128,
) -> Option<(u128, u128)> {
    let max_in = if swap_for_y {
        shift_div(U256::from(reserve_out), SCALE_OFFSET, price, true)?
    } else {
        mul_shift(U256::from(reserve_out), price, SCALE_OFFSET, true)?
    };
    let max_in: u128 = max_in.try_into().ok()?;
    let max_in = max_in.checked_add(fee_on(max_in, fee))?;
    if in_left >= max_in {
        return Some((max_in, reserve_out));
    }
    let net = U256::from(in_left - fee_from(in_left, fee));
    let out = if swap_for_y {
        mul_shift(net, price, SCALE_OFFSET, false)?
    } else {
        shift_div(net, SCALE_OFFSET, price, false)?
    };
    let out: u128 = out.try_into().ok()?;
    Some((in_left, out.min(reserve_out)))
}

/// getAmountsIn в одном бине. Возвращает: (вход с комиссией, выход). None — safe128.
fn amounts_in(
    price: U256,
    reserve_out: u128,
    fee: u32,
    swap_for_y: bool,
    out_left: u128,
) -> Option<(u128, u128)> {
    let out = reserve_out.min(out_left);
    let need = if swap_for_y {
        shift_div(U256::from(out), SCALE_OFFSET, price, true)?
    } else {
        mul_shift(U256::from(out), price, SCALE_OFFSET, true)?
    };
    let need: u128 = need.try_into().ok()?;
    Some((need.checked_add(fee_on(need, fee))?, out))
}

/// Итог: что отдали и что получили.
fn done(amount: u128, left: u128, unspecified: u128, exact_in: bool, complete: bool) -> Swapped {
    let (i, o) = if exact_in {
        (amount - left, unspecified)
    } else {
        (unspecified, amount - left)
    };
    Swapped {
        amount_in: U256::from(i),
        amount_out: U256::from(o),
        complete,
    }
}
