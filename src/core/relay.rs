//! Файл: core/relay.rs
//! Что делает: режим «Перегон» застрявшей монеты (решение 37): ищет площадку, где продажа выгоднее
//! с учётом всех затрат перевода (relay_value.rs: у биржи — справочник сетей ядра, минимумы вывода и депозита, налог
//! перевода; в USDT по живым ценам); площадки на паузе — не цели (решение 118, Я-14); лучше, чем здесь, —
//! переводит туда и пробует продать (sell.rs); перегонов больше stuck.max_relays или перегон не лучше —
//! продажа здесь — решение 220: убыток не выше stuck.relay_max_loss_pct — по рынку, иначе алерт (relay_here.rs).
//! Решение 220 (часть В): площадка, где лежит монета, на паузе — перегона (вывода) с неё нет, продажа здесь по тем же
//! правилам. Котировки продажи — кошельком сделки (здесь) и кошельком перегона (на цели).
//! Кто вызывает: stuck.rs.
//! От чего зависит: trade_ops.rs, transfers.rs, fills.rs, convert.rs, sell.rs, stuck.rs, registry (площадки монеты).
//! Решение 185: сделка биржа ↔ биржа без счёта DEX перегоняется на DEX только кошельком строки биржи, где лежит монета
//! (dex_account::for_relay; строки нет — DEX не цель), счёт записывается в сделку до перевода.
//! Решение 222: цель, выручка перегона на которую после затрат ≤ 0 (у relay_value приход-пыль на цели = 0 выручки), —
//! не цель; целей нет, а продажа здесь не считается или ≤ 0 — остаток списывается в пыль (dust::write_off), сделка
//! закрывается (живой случай PIT 08.11: «Перегон на MEXC (ожидается 0.0039 USDT)» остатка 770 PIT).
//! Настройки: stuck.max_relays, venue.quote_asset, stuck.relay_max_loss_pct (через relay_here.rs).

use rust_decimal::Decimal;

use crate::core::types::VenueId;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::deal::{Deal, Step};
use super::engine::{Core, lk, rd};

/// Площадки, куда монету можно перегнать отсюда (у монеты там есть рынок, приём и продажа через API; не на паузе).
fn targets(core: &Core, deal: &Deal) -> Vec<VenueId> {
    let coins = rd(&core.inner.coins);
    // Решение 131: по карте места, без прохода справочника.
    let Some(e) = super::ticker_index::find(core, &coins, &deal.direction.asset) else {
        return vec![];
    };
    let can_send = e.at(&deal.at).is_some_and(|p| p.can_send);
    if !can_send {
        return vec![];
    }
    // Решение 118 (Я-14): площадка на паузе (ошибки подряд, балансы не получены) — не цель перегона.
    let paused = lk(&core.inner.risk).venue_paused.clone();
    e.presence
        .iter()
        .filter(|p| p.excluded.is_empty() && p.can_receive && p.can_sell && p.venue != deal.at)
        .filter(|p| !paused.contains_key(&p.venue))
        // Решение 185: на DEX — только если есть кошелёк (сделки или строки биржи, где монета).
        .filter(|p| super::dex_account::for_relay(core, deal, &p.venue).is_ok())
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
    // Решение 220: продажа здесь — с порогом убытка stuck.relay_max_loss_pct (relay_here.rs), не по рынку вслепую.
    if deal.relays >= max {
        let why = format!("перегонов {max} — исчерпаны");
        return super::relay_here::sell_here(core, deal, &why).await;
    }
    // Решение 220 (часть В): площадка, где лежит монета, на паузе (риск: ошибки подряд, балансы не получены) — вывода
    // с неё не делаем (живой случай Bitget 40938 — каждый перегон с неё отказывал тем же отказом).
    let paused = lk(&core.inner.risk).venue_paused.get(&deal.at).cloned();
    if let Some(p) = paused {
        let why = format!(
            "площадка {} на паузе ({p}) — перегон с неё не делаем",
            deal.at.0
        );
        return super::relay_here::sell_here(core, deal, &why).await;
    }
    let a = deal.direction.asset.clone();
    // Решение 220: котировка — кошельком сделки.
    let here = super::trade_ops::deal_value(core, &deal)
        .await
        .map(|x| x.0)
        .ok();
    let mut best: Option<(VenueId, Decimal)> = None;
    // Решение 222: перегон, выручка которого после затрат ≤ 0 (или приход — пыль на площадке-цели), не делается.
    let mut tiny = false;
    for to in targets(core, &deal) {
        // Решение 220: продажа на цели котируется кошельком, которым пойдёт (решение 185: for_relay).
        let acc = super::dex_account::for_relay(core, &deal, &to)
            .ok()
            .flatten();
        match super::relay_value::value_at(core, &deal.at, &to, &a, deal.qty, acc.as_ref()).await {
            Ok(v) if v <= Decimal::ZERO => {
                tiny = true;
                let t = format!(
                    "{}: перегон {} {} на {} не окупается — ожидаемая выручка {v} USDT (меньше затрат перевода или приход — пыль там)",
                    deal.id, deal.qty, a.ticker, to.0
                );
                core.inner
                    .deps
                    .log(Level::Info, "core.relay", Some(&to.0), t, None)
                    .await
            }
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
    // Решение 222: перегонять не во что, а продажа здесь не считается или не окупается — остаток пыль.
    if super::dust::relay_dust(best.as_ref().map(|b| b.1), tiny, here) {
        let why =
            "остаток — пыль: перегон не окупается, продажа здесь не даёт выручки (решение 222)";
        super::dust::write_off(core, &mut deal, why).await;
        return super::ledger::finish(core, &mut deal, Step::Closed, why).await;
    }
    let Some((to, v)) = best.filter(|(_, v)| here.is_none_or(|h| *v > h)) else {
        return super::relay_here::sell_here(core, deal, "перегон не выгоднее продажи здесь").await;
    };
    let from = deal.at.clone();
    // Решение 185: сделка биржа ↔ биржа перегоняется на DEX — дальше она ведётся кошельком строки биржи, где монета
    // (перевод, продажа на DEX, возврат USDT): счёт записывается в сделку до перевода.
    if deal.dex_account.is_none()
        && let Ok(Some(acc)) = super::dex_account::for_relay(core, &deal, &to)
    {
        deal.note(format!(
            "Перегон на DEX — кошелёк {} (строка {} в маппинге)",
            acc.0, from.0
        ));
        deal.dex_account = Some(acc);
    }
    deal.step = Step::Transfer;
    deal.note(format!(
        "Перегон на {} (ожидается {})",
        crate::notify::human::venue(&to.0),
        crate::notify::human::usd(v)
    ));
    super::ledger::save(core, &deal).await;
    let price = deal.buy.as_ref().map(|b| b.avg_price).unwrap_or_default();
    match super::transfers::send(core, &deal.id, &from, &to, &a, deal.qty).await {
        Ok((sent, credited)) => {
            super::fills::transfer(core, &mut deal, &sent, &credited, &to, price).await;
            deal.relays += 1;
            let text = crate::notify::human::Msg::new(
                crate::notify::human::INFO,
                "Застрявшая монета перегнана",
            )
            .line(format!(
                "{} {} {}",
                crate::notify::human::qty(deal.qty),
                a.ticker,
                crate::notify::human::route(&from.0, &to.0)
            ))
            .kv("Перегон", format!("{}-й", deal.relays))
            .id(&deal.id)
            .render();
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
