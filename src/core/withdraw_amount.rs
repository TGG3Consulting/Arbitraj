//! A withdrawal with an externally charged fixed fee debits its on-chain amount plus the fee.
//! The amount and network limits in the POST apply after the fee is reserved.
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use super::size::Fail;
use super::types::CoinNetworkInfo;

#[derive(Debug, PartialEq, Eq)]
pub struct Amounts {
    pub wire: Decimal,
    pub fee: Decimal,
    pub debit: Decimal,
}

pub fn amounts(budget: Decimal, info: &CoinNetworkInfo) -> Result<Amounts, Fail> {
    let fee = info
        .withdraw_fee
        .filter(|f| *f >= Decimal::ZERO)
        .filter(|_| info.withdraw_fee_pct.unwrap_or_default().is_zero())
        .ok_or_else(|| Fail::Stop("отсутствует фиксированная комиссия вывода".into()))?;
    if budget <= fee {
        return Err(Fail::TooSmall(format!(
            "бюджет {budget} не покрывает комиссию вывода {fee}"
        )));
    }
    let wire = round_wire(budget - fee, info)?;
    if wire <= Decimal::ZERO || info.withdraw_min.is_some_and(|min| wire < min) {
        return Err(Fail::TooSmall(format!(
            "сумма перевода {wire} после комиссии меньше минимума вывода"
        )));
    }
    if info
        .withdraw_max
        .is_some_and(|max| max > Decimal::ZERO && wire > max)
    {
        return Err(Fail::TooLarge(format!(
            "сумма перевода {wire} после комиссии больше максимума вывода"
        )));
    }
    let debit = wire
        .checked_add(fee)
        .filter(|d| *d <= budget && d.checked_sub(fee) == Some(wire))
        .ok_or_else(|| Fail::Stop("сумма и комиссия не помещаются в бюджет вывода".into()))?;
    Ok(Amounts { wire, fee, debit })
}

fn round_wire(mut wire: Decimal, info: &CoinNetworkInfo) -> Result<Decimal, Fail> {
    if let Some(p) = info.withdraw_precision {
        wire = wire.trunc_with_scale(p);
    }
    if let Some(step) = info.withdraw_step.filter(|s| *s > Decimal::ZERO) {
        wire -= wire
            .checked_rem(step)
            .ok_or_else(|| Fail::Stop("невозможно округлить сумму вывода до шага".into()))?;
    }
    if info
        .withdraw_precision
        .is_some_and(|p| wire != wire.trunc_with_scale(p))
    {
        return Err(Fail::Stop("несогласованные точность и шаг вывода".into()));
    }
    Ok(wire)
}

pub fn for_network(
    budget: Decimal,
    info: Option<&CoinNetworkInfo>,
) -> Result<Option<Amounts>, Fail> {
    info.filter(|i| i.use_net_withdrawal_budget)
        .map(|i| amounts(budget, i))
        .transpose()
}

/// Minimal number of fees for a group. Distribute the wires on the network
/// grid; leave less than one wire step of unspent budget in the final part.
pub fn group_parts(total: Decimal, info: &CoinNetworkInfo) -> Result<Vec<Decimal>, Fail> {
    match amounts(total, info) {
        Err(Fail::TooLarge(_)) => {}
        result => return result.map(|_| vec![total]),
    }
    let fee = info
        .withdraw_fee
        .ok_or_else(|| Fail::Stop("неизвестная комиссия вывода".into()))?;
    let max = round_wire(info.withdraw_max.unwrap_or_default(), info)?;
    if max <= Decimal::ZERO {
        return Err(Fail::Stop("лимит вывода меньше одного шага".into()));
    }
    let capacity = max
        .checked_add(fee)
        .ok_or_else(|| Fail::Stop("слишком большой лимит вывода".into()))?;
    let too_many = || Fail::Stop("число частей возврата не помещается в память".into());
    let remainder = total.checked_rem(capacity).ok_or_else(too_many)?;
    let mut count = (total - remainder)
        .checked_div(capacity)
        .ok_or_else(too_many)?;
    if round_wire(remainder, info)? > Decimal::ZERO {
        count = count.checked_add(Decimal::ONE).ok_or_else(too_many)?;
    }
    let mut left = total;
    let mut out = Vec::new();
    out.try_reserve_exact(count.to_usize().ok_or_else(too_many)?)
        .map_err(|_| too_many())?;
    while count > Decimal::ONE {
        // Reserve a valid equal share for later requests. If rounding all later
        // shares down would leave too much for the last one, pay the larger first.
        let fees = count
            .checked_mul(fee)
            .filter(|f| *f < left)
            .ok_or_else(|| {
                Fail::TooSmall("бюджет группы не покрывает комиссии всех частей".into())
            })?;
        let remaining_wire = round_wire(left - fees, info)?;
        let base = round_wire(remaining_wire / count, info)?;
        let wire = (remaining_wire - base * (count - Decimal::ONE)).min(max);
        let piece = amounts(wire + fee, info)?.debit;
        out.push(piece);
        left -= piece;
        count -= Decimal::ONE;
    }
    amounts(left, info)?;
    out.push(left);
    Ok(out)
}
