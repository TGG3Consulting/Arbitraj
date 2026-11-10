//! Файл: core/relay.rs
//! Что делает: режим «Перегон» застрявшей монеты (решение 37): ищет площадку, где продажа выгоднее
//! с учётом всех затрат перевода (relay_value.rs: у биржи — справочник сетей ядра, минимумы вывода и депозита, налог
//! перевода; в USDT по живым ценам); площадки на паузе — не цели (решение 118, Я-14); лучше, чем здесь, —
//! переводит туда и пробует продать (sell.rs); перегонов больше stuck.max_relays или перегон не лучше —
//! продажа здесь — решение 220: убыток не выше stuck.relay_max_loss_pct — по рынку, иначе алерт (relay_here.rs).
//! Решение 220 (часть В): площадка, где лежит монета, на паузе — перегона (вывода) с неё нет, продажа здесь по тем же
//! правилам. Котировки продажи — кошельком сделки (здесь) и кошельком перегона (на цели).
//! Решение 220 (Тигран 08.11): перегон не состоялся (отказ вывода) — монеты на месте: продажа здесь по тем же правилам,
//! вывод не повторяется; монет нет или исход неизвестен — сделка остановлена (relay_here::transfer_failed).
//! Кто вызывает: stuck.rs.
//! От чего зависит: trade_ops.rs, transfers.rs, fills.rs, convert.rs, sell.rs, stuck.rs, stuck_quote.rs, registry (площадки
//! монеты). Решение 238 (ревью, п. 2): котировки «продать здесь» нет из-за незаданной настройки — сделка остановлена.
//! Решение 185: сделка биржа ↔ биржа без счёта DEX перегоняется на DEX только кошельком строки биржи, где лежит монета
//! (dex_account::for_relay; строки нет — DEX не цель), счёт записывается в сделку до перевода.
//! Решение 222: цель, выручка перегона на которую после затрат ≤ 0 (у relay_value приход-пыль на цели = 0 выручки), —
//! не цель; целей нет, а продажа здесь не считается или ≤ 0 — остаток списывается в пыль (dust::write_off), сделка
//! закрывается (живой случай PIT 08.11: «Перегон на MEXC (ожидается 0.0039 USDT)» остатка 770 PIT).
//! Решение 230: кошелёк DEX на паузе от сверки (wallet_pause.rs) — не цель перегона; монета на нём — продажа на месте.
//! Решение 227 (дополнение): площадка уже отказала в выводе в этой сделке (Deal.withdraw_refused) — перегона с неё нет,
//! продажа здесь (relay_here.rs); отказ вывода перегона ставит эту метку (withdraw_refused.rs).
//! Решение 227: перегон на DEX — через send_trade (зачисление по приходу в блокчейне, решение 151), как перевод сделки.
//! Решение 249 (живой случай Тиграна 10.11: сделка BingX → DEX, перегон увёз монету на MEXC с venue.cex_cex = нет):
//! биржа-цель, запрещённая правилами биржа ↔ биржа (venue.cex_cex у биржи пути монеты или у цели, список
//! venue.cex_cex_deny бирж пути монеты — cex_link::relay_block), не цель; разрешённых целей нет — продажа здесь.
//! Дополнение 10.11 (п. 5): отсеянные правилом биржи — одной строкой в хронологию сделки на каждый перегон (биржа и
//! причина каждой), как в «Продать и перегнать».
//! Решение 247, дополнение 10.11 (Тигран: согласовано): продажа на цели сразу после перегона (sell::sell_after_relay) —
//! выгодно (итог ≥ 0) → по рынку с пере-замером продажи DEX при откате «мало на выходе» в пределах того же условия
//! (порог убытка 0, remeasure::PROFITABLE); невыгодно — режим застрявшей монеты, как раньше. Продажа здесь — relay_here.rs
//! (порог stuck.relay_max_loss_pct — он же граница пере-замера, решение 247).
//! Настройки: stuck.max_relays, venue.quote_asset, stuck.relay_max_loss_pct (через relay_here.rs); venue.cex_cex,
//! venue.cex_cex_deny (через cex_link.rs).

use rust_decimal::Decimal;

use crate::core::types::VenueId;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::deal::{Deal, Step};
use super::engine::{Core, lk, rd};

/// Площадки, куда монету можно перегнать отсюда (у монеты там есть рынок, приём и продажа через API; не на паузе;
/// решение 249 — разрешены правилами биржа ↔ биржа). Открыта для тестов (tests/core_cex_cex_deny.rs). Ошибок нет.
pub fn targets(core: &Core, deal: &Deal) -> Vec<VenueId> {
    targets_why(core, deal).0
}

/// Как targets, плюс биржи, отсеянные правилом биржа ↔ биржа (решение 249, дополнение 10.11, п. 5), — «биржа: причина».
/// Открыта для тестов. Принимает: ядро, сделку. Возвращает: (цели, отсеянные правилом). Ошибок нет.
pub fn targets_why(core: &Core, deal: &Deal) -> (Vec<VenueId>, Vec<String>) {
    let coins = rd(&core.inner.coins);
    // Решение 131: по карте места, без прохода справочника.
    let Some(e) = super::ticker_index::find(core, &coins, &deal.direction.asset) else {
        return (vec![], vec![]);
    };
    let can_send = e.at(&deal.at).is_some_and(|p| p.can_send);
    if !can_send {
        return (vec![], vec![]);
    }
    // Решение 118 (Я-14): площадка на паузе (ошибки подряд, балансы не получены) — не цель перегона.
    let paused = lk(&core.inner.risk).venue_paused.clone();
    let mut ruled = vec![];
    let out = e
        .presence
        .iter()
        .filter(|p| p.excluded.is_empty() && p.can_receive && p.can_sell && p.venue != deal.at)
        .filter(|p| !paused.contains_key(&p.venue))
        // Решение 249: биржа-цель запрещена правилами биржа ↔ биржа относительно пути монеты (cex_link.rs).
        .filter(
            |p| match super::cex_link::relay_block(core, deal, &p.venue) {
                Some(w) => {
                    ruled.push(format!("{}: {w}", p.venue.0));
                    false
                }
                None => true,
            },
        )
        // Решение 185: на DEX — только если есть кошелёк (сделки или строки биржи, где монета); решение 230 — и этот
        // кошелёк не на паузе от сверки.
        .filter(
            |p| match super::dex_account::for_relay(core, deal, &p.venue) {
                Ok(acc) => super::wallet_pause::paused(core, acc.as_ref()).is_none(),
                Err(_) => false,
            },
        )
        .map(|p| p.venue.clone())
        .collect();
    (out, ruled)
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
    // Решение 230: монета на кошельке DEX на паузе от сверки — перегона с него нет, как с площадки на паузе.
    if let Some(p) = super::wallet_pause::paused_at(core, &deal) {
        let why = format!(
            "{} на паузе ({}) — перегон с него не делаем",
            p.who(),
            p.why
        );
        return super::relay_here::sell_here(core, deal, &why).await;
    }
    // Решение 227 (дополнение): площадка, где лежит монета, уже отказала в выводе в этой сделке (адресная книга и т. п.)
    // — перегон с неё заведомо откажет: продажа здесь по тем же правилам.
    if super::withdraw_refused::refused(&deal, &deal.at) {
        let why = format!(
            "вывод с {} в этой сделке уже отклонён площадкой — перегон с неё не делаем",
            deal.at.0
        );
        return super::relay_here::sell_here(core, deal, &why).await;
    }
    let a = deal.direction.asset.clone();
    // Решение 220: котировка — кошельком сделки.
    // Решение 238 (ревью, п. 2): котировки здесь нет, потому что не задана настройка (NotConfigured, например
    // venue.book_stale_ms площадки), — сделка остановлена с причиной (stuck_quote::halt_not_configured), а не перегон вслепую.
    let here = match super::trade_ops::deal_quote(core, &deal).await {
        Ok((v, _, _)) => Some(v),
        Err(e) if super::stuck_quote::not_configured(&e) => {
            return super::stuck_quote::halt_not_configured(core, deal, &e).await;
        }
        Err(_) => None,
    };
    let mut best: Option<(VenueId, Decimal)> = None;
    // Решение 222: перегон, выручка которого после затрат ≤ 0 (или приход — пыль на площадке-цели), не делается.
    let mut tiny = false;
    let (list, ruled) = targets_why(core, &deal);
    // Решение 249, дополнение 10.11 (п. 5): отсеянные правилом биржи — одной строкой в хронологию сделки.
    if !ruled.is_empty() {
        deal.note(format!(
            "Перегон: не цели по правилу биржа ↔ биржа (решение 249) — {}",
            ruled.join("; ")
        ));
        super::ledger::save(core, &deal).await;
    }
    for to in list {
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
    // Решение 227 (п. 4): перегон на кошелёк DEX засчитывается по приходу монеты в блокчейне, как перевод сделки
    // (решение 151: send_trade → credit_chain; кошелёк — счёт сделки, записанный выше, решение 185), а не по хешу от
    // биржи-отправителя (живой случай JCT: +18 с). На биржу — как раньше (send_trade без DEX = send).
    let mut gas = Vec::new();
    let res =
        super::transfers::send_trade(core, &deal.id, (&from, &to), &a, deal.qty, &mut gas).await;
    // Решение 236, дополнение: газ неудачных переводов из кошелька DEX (откат, отмена) — в затраты сделки.
    super::deal_gas::charge(core, &mut deal, &from, &mut gas).await;
    match res {
        Ok((sent, credited, chain)) => {
            if let Some(line) = chain {
                deal.note(line);
            }
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
            // Решение 247, дополнение 10.11: продажа на цели — с пере-замером продажи DEX по условию «выгодно».
            Box::pin(super::sell::sell_after_relay(core, deal)).await;
        }
        // Исход неизвестен (вывод мог уйти) — ни продажи, ни повтора: ручная проверка.
        Err(e) if e.outcome_unknown() => {
            super::ledger::finish(
                core,
                &mut deal,
                Step::Halted,
                &format!("перегон не состоялся: {e}"),
            )
            .await
        }
        // Решение 220 (Тигран 08.11, п. 2): отказ вывода — монеты на месте: продажа здесь по порогу, вывод не
        // повторяем; монет на месте нет — остановка, решает человек (relay_here::transfer_failed).
        Err(e) => {
            deal.step = Step::Stuck;
            // Решение 227: отказ вывода площадкой — метка, следующий «Перегон» с неё не пойдёт.
            super::withdraw_refused::mark_at(core, &mut deal, &from, &e);
            // Решение 253 (г): отказ запоминается по паре «монета + биржа» — новые входы с неё закрыты.
            let asset = deal.direction.asset.clone();
            super::withdraw_block::close(core, &deal.id, &asset, &from, &e).await;
            let why = format!("перегон не состоялся: {e}");
            deal.note(format!("{why} — продажа здесь, вывод не повторяем"));
            super::relay_here::transfer_failed(core, deal, &why).await
        }
    }
}
