//! Файл: core/relay.rs
//! Что делает: режим «Перегон» застрявшей монеты (решение 37): ищет площадку, где продажа выгоднее
//! с учётом всех затрат перевода (стоимость вывода через шлюз, в USDT по живым ценам); лучше, чем здесь, —
//! переводит туда и пробует продать (sell.rs); перегонов больше stuck.max_relays или перегон не лучше —
//! продажа по рынку здесь.
//! Кто вызывает: stuck.rs.
//! От чего зависит: trade_ops.rs, transfers.rs, fills.rs, convert.rs, sell.rs, stuck.rs, registry (площадки монеты).
//! Настройки: stuck.max_relays, venue.quote_asset.

use rust_decimal::Decimal;

use crate::core::types::{Asset, VenueId};
use crate::gateway::contract::VenueKind;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::convert::{self, Rates};
use super::deal::{Deal, Step};
use super::engine::{Core, rd};

/// Чистая выручка (USDT) при перегоне qty на площадку `to` и продаже там. Ошибка — текст причины.
async fn value_at(
    core: &Core,
    from: &VenueId,
    to: &VenueId,
    a: &Asset,
    qty: Decimal,
) -> Result<Decimal, String> {
    let gw = core.inner.deps.gw.get(from).await.map_err(|e| e.message)?;
    let net = super::transfers::network(core, a, from)?;
    let costs = gw
        .withdraw_cost(a, &net, qty)
        .await
        .map_err(|e| e.message)?;
    let coin: Decimal = costs
        .iter()
        .filter(|c| c.asset == a.ticker)
        .map(|c| c.amount)
        .sum();
    let (v, q) = super::trade_ops::sell_value(core, to, a, qty - coin).await?;
    let quote = core
        .inner
        .deps
        .settings
        .text(
            "venue.quote_asset",
            At {
                venue: Some(&from.0),
                coin: None,
            },
        )
        .map_err(|e| e.message)?;
    let bnb = convert::bnb_total(&costs);
    let cex: Vec<VenueId> = rd(&core.inner.kinds)
        .iter()
        .filter(|(_, k)| *k == VenueKind::Cex)
        .map(|(v, _)| v.clone())
        .collect();
    let rates = Rates {
        quote: vec![quote],
        coin: a.ticker.clone(),
        coin_price: q.avg_price,
        bnb: if bnb.is_zero() {
            None
        } else {
            Some(convert::bnb_price(&core.inner.deps, &cex, bnb).await?)
        },
    };
    let mut out = v;
    for c in costs.iter().filter(|c| c.asset != a.ticker) {
        out -= convert::to_usdt(c, &rates)?;
    }
    Ok(out)
}

/// Площадки, куда монету можно перегнать отсюда (у монеты там есть рынок и приём).
fn targets(core: &Core, deal: &Deal) -> Vec<VenueId> {
    let coins = rd(&core.inner.coins);
    let Some(e) = coins.iter().find(|c| c.asset == deal.direction.asset) else {
        return vec![];
    };
    let can_send = e.at(&deal.at).is_some_and(|p| p.can_send);
    if !can_send {
        return vec![];
    }
    e.presence
        .iter()
        .filter(|p| p.excluded.is_empty() && p.can_receive && p.venue != deal.at)
        .map(|p| p.venue.clone())
        .collect()
}

/// Режим «Перегон». Ошибок нет.
pub async fn run(core: &Core, mut deal: Deal) {
    let max = match core
        .inner
        .deps
        .settings
        .integer("stuck.max_relays", At::default())
    {
        Ok(v) => v,
        Err(e) => return super::ledger::finish(core, &mut deal, Step::Halted, &e.message).await,
    };
    if deal.relays >= max {
        return super::stuck::sold_by_market(core, deal, &format!("перегонов {max} — исчерпаны"))
            .await;
    }
    let a = deal.direction.asset.clone();
    let here = super::trade_ops::sell_value(core, &deal.at, &a, deal.qty)
        .await
        .map(|x| x.0)
        .ok();
    let mut best: Option<(VenueId, Decimal)> = None;
    for to in targets(core, &deal) {
        match value_at(core, &deal.at, &to, &a, deal.qty).await {
            Ok(v) if best.as_ref().is_none_or(|(_, b)| v > *b) => best = Some((to, v)),
            Ok(_) => {}
            Err(e) => {
                core.inner
                    .deps
                    .log(
                        Level::Info,
                        "core.relay",
                        Some(&to.0),
                        format!("{}: перегон на {} не оценён: {e}", deal.id, to.0),
                        None,
                    )
                    .await
            }
        }
    }
    let Some((to, v)) = best.filter(|(_, v)| here.is_none_or(|h| *v > h)) else {
        return super::stuck::sold_by_market(core, deal, "перегон не выгоднее продажи здесь").await;
    };
    let from = deal.at.clone();
    deal.step = Step::Transfer;
    deal.note(format!(
        "Перегон на {} (ожидается {} USDT)",
        to.0,
        v.round_dp(4)
    ));
    super::ledger::save(core, &deal).await;
    let price = deal.buy.as_ref().map(|b| b.avg_price).unwrap_or_default();
    match super::transfers::send(core, &deal.id, &from, &to, &a, deal.qty).await {
        Ok((sent, credited)) => {
            super::fills::transfer(core, &mut deal, &sent, &credited, &to, price).await;
            deal.relays += 1;
            let text = format!(
                "{}: {} {} перегнаны {} → {} ({}-й раз)",
                deal.id, deal.qty, a.ticker, from.0, to.0, deal.relays
            );
            core.inner
                .deps
                .notify("stuck.relayed", Some(&to.0), &text)
                .await;
            deal.step = Step::Selling;
            super::ledger::save(core, &deal).await;
            Box::pin(super::sell::sell_or_stuck(core, deal)).await;
        }
        Err(e) => {
            super::ledger::finish(
                core,
                &mut deal,
                Step::Halted,
                &format!("перегон не состоялся: {e}"),
            )
            .await
        }
    }
}
