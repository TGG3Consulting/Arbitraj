//! Последовательные обмены одного зарегистрированного пула Ranges. Vault хранит целые
//! единицы токена, хук обновляет виртуальные остатки с точностью 18 знаков.
use super::{
    balancer_math::{div_down, mul_down, mul_up, one},
    puzzle_ranges,
};
use crate::core::types::pool_vault::{V3Hook, VaultCurve, VaultPoolData, VaultV3Rule};
use alloy_primitives::U256;

pub fn supported(d: &VaultPoolData) -> bool {
    d.v3.as_ref().is_some_and(|v|!d.paused && matches!(v.hook,V3Hook::None) &&
        matches!(v.rule,VaultV3Rule::PuzzleRanges{enabled:true,sequence_fee:Some(p),..} if p<=one())
        && v.rates.iter().all(|r|*r==one()) && v.via.iter().all(Option::is_none))
}

/// Цена и новое состояние после одного фактически исполненного EXACT_IN.
pub fn advance(
    d: &VaultPoolData,
    i: usize,
    j: usize,
    input: U256,
) -> Option<(U256, VaultPoolData)> {
    if !supported(d) || i == j || input.is_zero() {
        return None;
    }
    let mut next = d.clone();
    let v = next.v3.as_mut()?;
    let VaultV3Rule::PuzzleRanges {
        virtual_balances,
        sequence_fee: Some(protocol),
        ..
    } = &mut v.rule
    else {
        return None;
    };
    let VaultCurve::Weighted { weights } = &next.curve else {
        return None;
    };
    let si = *next.scaling.get(i)?;
    let so = *next.scaling.get(j)?;
    let scaled = input.checked_mul(si)?;
    let fee = mul_up(scaled, next.fee)?;
    let net = scaled.checked_sub(fee)?;
    if net < v.min_trade {
        return None;
    }
    let out = puzzle_ranges::on_swap(virtual_balances, &v.live, weights, i, j, net, true)?;
    if out < v.min_trade {
        return None;
    }
    let payout = out.checked_div(so)?;
    // Vault.sol _computeAndChargeAggregateSwapFees rounds to raw before the protocol split.
    let fee_raw = div_down(fee, si.checked_mul(one())?)?;
    let charged = mul_down(fee_raw, *protocol)?;
    next.balances[i] = next
        .balances
        .get(i)?
        .checked_add(input.checked_sub(charged)?)?;
    next.balances[j] = next.balances.get(j)?.checked_sub(payout)?;
    v.live[i] = next.balances[i].checked_mul(si)?;
    v.live[j] = next.balances[j].checked_mul(so)?;
    virtual_balances[i] = virtual_balances
        .get(i)?
        .checked_add(scaled.checked_sub(mul_down(fee, *protocol)?)?)?;
    virtual_balances[j] = virtual_balances.get(j)?.checked_sub(out)?;
    Some((payout, next))
}
