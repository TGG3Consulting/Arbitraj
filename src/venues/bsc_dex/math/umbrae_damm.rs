//! Umbrae DAMM. Расчёт swap по фактическому приходу в пару и переход обоих резервов,
//! ставки, накопленных комиссий и индекса LP по DAMMPairUpgradeable.

use super::{full_math, swap_cl::Swapped};
use crate::core::types::UmbraeDammState;
use alloy_primitives::{Address, U256};

const BPS: u16 = 10_000;
const PRICE_SCALE: u64 = 1_000_000_000_000_000_000;

pub struct Fill {
    pub swapped: Swapped,
    pub fee: U256,
    pub after: UmbraeDammState,
}

pub fn rate(s: &UmbraeDammState) -> Option<u16> {
    if s.base_fee > s.max_fee || s.max_fee > BPS || s.protocol_share > BPS || s.volatility > BPS {
        return None;
    }
    let variable = u32::from(s.base_fee) * u32::from(s.volatility) / u32::from(BPS);
    Some((u32::from(s.base_fee) + variable).min(u32::from(s.max_fee)) as u16)
}

fn charge(value: U256, bps: u16) -> Option<U256> {
    Some(value.checked_mul(U256::from(bps))? / U256::from(BPS))
}

/// Наименьшая положительная сумма n с n − floor(n·bps/10000) >= target.
fn gross(target: U256, bps: u16) -> Option<U256> {
    let adjusted = full_math::mul_div(
        target.checked_sub(U256::ONE)?,
        U256::from(BPS),
        U256::from(BPS.checked_sub(bps)?),
    )?;
    adjusted.checked_add(U256::ONE)
}

fn accrue(s: &mut UmbraeDammState, fee: U256) -> Option<()> {
    if fee.is_zero() {
        return Some(());
    }
    let protocol = charge(fee, s.protocol_share)?;
    let lp = fee - protocol;
    if s.lp_supply.is_zero() {
        s.protocol_fees = s.protocol_fees.checked_add(fee)?;
    } else {
        s.protocol_fees = s.protocol_fees.checked_add(protocol)?;
        s.lp_fees = s.lp_fees.checked_add(lp)?;
        let increase = lp.checked_mul(U256::from(PRICE_SCALE))? / s.lp_supply;
        s.lp_index = s.lp_index.checked_add(increase)?;
    }
    Some(())
}

fn sync(s: &mut UmbraeDammState) -> Option<()> {
    let f = s.fee_side;
    s.reserves = s.balances;
    s.reserves[f] = s.balances[f].checked_sub(s.protocol_fees.checked_add(s.lp_fees)?)?;
    if s.reserves.iter().any(|n| n.bit_len() > 112) {
        return None;
    }
    if !s.reserves[0].is_zero() {
        let price = s.reserves[1].checked_mul(U256::from(PRICE_SCALE))? / s.reserves[0];
        if !s.last_price.is_zero() {
            let diff = price.max(s.last_price) - price.min(s.last_price);
            let delta = diff.checked_mul(U256::from(BPS))? / s.last_price;
            let decayed = u32::from(s.volatility) * 9000 / u32::from(BPS);
            s.volatility = delta
                .checked_add(U256::from(decayed))?
                .min(U256::from(BPS))
                .to::<u16>();
        }
        s.last_price = price;
    }
    Some(())
}

/// amount при exact-in — прирост баланса входного токена именно от плательщика.
/// При запросе цели возвращаем минимальный такой вход и полную фактическую выплату.
pub fn fill(s: &UmbraeDammState, input: Address, exact_in: bool, amount: U256) -> Option<Fill> {
    if amount.is_zero()
        || s.fee_side >= 2
        || s.tokens[0] == s.tokens[1]
        || s.tokens.contains(&Address::ZERO)
        || s.reserves.iter().any(|n| n.is_zero() || n.bit_len() > 112)
    {
        return None;
    }
    let i = s.tokens.iter().position(|&t| t == input)?;
    let j = 1 - i;
    let bps = rate(s)?;
    let fee_on_input = s.fee_side == i;
    if fee_on_input {
        s.balances[i].checked_sub(s.protocol_fees.checked_add(s.lp_fees)?)?;
    }
    let spent = if exact_in {
        amount
    } else {
        let desired = if fee_on_input {
            amount
        } else {
            gross(amount, bps)?
        };
        let effective =
            full_math::mul_div_up(desired, s.reserves[i], s.reserves[j].checked_sub(desired)?)?;
        if fee_on_input {
            gross(effective, bps)?
        } else {
            effective
        }
    };
    let input_fee = if fee_on_input {
        charge(spent, bps)?
    } else {
        U256::ZERO
    };
    let effective = spent - input_fee;
    let output = effective.checked_mul(s.reserves[j])? / s.reserves[i].checked_add(effective)?;
    let fee = if fee_on_input {
        input_fee
    } else {
        charge(output, bps)?
    };
    let payout = if fee_on_input { output } else { output - fee };
    if payout.is_zero() || (!exact_in && payout < amount) {
        return None;
    }

    let mut after = s.clone();
    after.balances[i] = s.balances[i].checked_add(spent)?;
    after.balances[j] = s.balances[j].checked_sub(payout)?;
    accrue(&mut after, fee)?;
    sync(&mut after)?;
    Some(Fill {
        swapped: Swapped {
            amount_in: spent,
            amount_out: payout,
            complete: true,
        },
        fee,
        after,
    })
}

/// Глубина по решению 73 без комиссии. Измеряется в расходуемом токене
/// на покупке и в выплаченном на продаже, с ограничением по физическим остаткам.
pub fn band(s: &UmbraeDammState, i: usize, target: U256, buy: bool) -> Option<(U256, bool)> {
    let j = 1usize.checked_sub(i)?;
    let mut balances = s.balances;
    let fee_balance = balances.get_mut(s.fee_side)?;
    *fee_balance = fee_balance.checked_sub(s.protocol_fees.checked_add(s.lp_fees)?)?;
    let max_reserve = (U256::ONE << 112usize) - U256::ONE;
    let room = max_reserve.saturating_sub(balances[i]);
    let (ri, ro) = (s.reserves[i], s.reserves[j]);
    if ri.is_zero() || ro.is_zero() {
        return Some((U256::ZERO, true));
    }
    // Цена использует сохранённые резервы. После перевода физический выход обязан
    // поместиться под uint112, вход не может израсходовать уже начисленные комиссии.
    let output_cap = balances[j].min(ro - U256::ONE);
    let mut largest = room;
    if output_cap < ro - U256::ONE {
        let next = output_cap + U256::ONE;
        largest = largest.min(full_math::mul_div_up(next, ri, ro - next)?.checked_sub(U256::ONE)?);
    }
    let wanted = if buy {
        target
    } else {
        full_math::mul_div_up(target, ri, ro.checked_sub(target)?)?
    };
    let spent = wanted.min(largest);
    let payout = full_math::mul_div(spent, ro, ri.checked_add(spent)?)?;
    if payout.is_zero() || balances[j].checked_sub(payout)? > max_reserve {
        return Some((U256::ZERO, false));
    }
    Some((if buy { spent } else { payout }, spent == wanted))
}

#[cfg(test)]
#[path = "umbrae_damm_tests.rs"]
mod tests;
