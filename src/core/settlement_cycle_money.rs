//! Файл: core/settlement_cycle_money.rs.
//! Что делает: сохраняет остатки и переводит подтверждённые сторонние затраты в валюту учёта.
//! Кто вызывает: исполнитель полного денежного пути.
//! От чего зависит: суммы ордера, факты вывода и котировка на исходной бирже.
//! Настройки: шаги спотовой пары.
use super::{
    engine::Core,
    settlement_cycle::Mandate,
    settlement_transfer::Movement,
    types::{CostKind, Direction, Fee},
};
use crate::gateway::settlement_intent::SpotFill;
use rust_decimal::Decimal;

pub(super) fn sell_qty(
    core: &Core,
    d: &Direction,
    available: Decimal,
    price: Decimal,
) -> Result<Decimal, String> {
    let ticker = super::venue_ticker::ticker_on(core, &d.asset, &d.sell_on);
    let rules = super::dirctx_venue::pair_rules(core, &d.sell_on, &ticker)?;
    let Some(r) = rules else { return Ok(available) };
    let qty = available
        .checked_rem(r.step_size)
        .and_then(|tail| available.checked_sub(tail))
        .ok_or("Нет шага продажи на бирже")?;
    super::costs_quote::order_limits(&d.sell_on.0, Some(&r), qty, price)
        .map_err(|e| e.to_string())?;
    Ok(qty)
}

pub(super) fn withdrawal_remainder(budget: Decimal, m: &Movement) -> Result<Decimal, String> {
    let debit = m
        .debited
        .ok_or("У зафиксированного вывода отсутствует сумма списания отправителя")?;
    budget
        .checked_sub(debit)
        .filter(|v| *v >= Decimal::ZERO)
        .ok_or("Вывод превысил принадлежащую сделке сумму".into())
}

pub(super) async fn fees(
    core: &Core,
    p: &Mandate,
    buy: &SpotFill,
    sell: &SpotFill,
    target: &Movement,
    cash: Option<&Movement>,
) -> Result<Decimal, String> {
    let m = &p.market;
    let mut fees = Vec::new();
    for execution in [buy, sell] {
        let t = &execution.trade;
        for f in &t.fees {
            if t.chain.is_some() || (f.asset != t.request.asset.ticker && f.asset != m.quote) {
                fees.push(f.clone());
            }
        }
        if !super::deal_gas::of_filled(t).is_empty() {
            return Err(
                "В источнике появились дополнительные транзакции, нужна отдельная сверка комиссий"
                    .into(),
            );
        }
    }
    for movement in std::iter::once(target).chain(cash) {
        let c = movement
            .sent
            .fee
            .as_ref()
            .ok_or("Площадка не сообщила комиссию подтверждённого перевода")?;
        if c.kind != CostKind::WithdrawFee || c.asset != movement.sent.asset.ticker {
            fees.push(Fee {
                asset: c.asset.clone(),
                amount: c.amount,
                usd: None,
            });
        }
    }
    let mut total = Decimal::ZERO;
    for f in fees {
        if f.amount.is_zero() {
            continue;
        }
        if f.amount < Decimal::ZERO {
            return Err("Неоднозначная отрицательная комиссия".into());
        }
        let value = if f.asset == m.quote {
            f.amount
        } else {
            let price = super::convert::price_of(
                &core.inner.deps,
                std::slice::from_ref(&m.cex),
                &f.asset,
                f.amount,
            )
            .await?;
            f.amount
                .checked_mul(price)
                .ok_or("Оценка фактической комиссии вне диапазона")?
        };
        total = total.checked_add(value).ok_or("Комиссии вне диапазона")?;
    }
    Ok(total)
}
