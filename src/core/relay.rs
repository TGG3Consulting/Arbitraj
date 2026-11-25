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
//! Решение 413 (ревью t198, Б2): перед оценкой и выводом с биржи — доля сделки по факту счёта (sell_fact::before).
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
//! Решение 415 (t200): котировки «продать здесь» — под ходом очереди продажи счёта на время котировки (sell_turn_fresh::once).
//! Ревью t200 (М-3): котировка до чтения факта не берётся, если после неё на ключе прошла продажа (sell_turn::sold_after).
//! Решение 417 (Тигран 19.11, живые THE, SNDKB, SPCXB, SINGULARRY): продажа здесь выгодна (итог сделки ≥ 0) — цели не
//! ищутся, продажа здесь по рынку с пере-замером по условию «выгодно» (remeasure::PROFITABLE); иначе оценка каждой цели —
//! с запасом на движение цены за перевод, как у плана входа (relay_reserve.rs), перегон — только если оценка с запасом
//! больше выручки здесь; цель без запаса (σ не измерена, настройка не задана) не оценена. Расчёт запаса — в строке
//! хронологии «Перегон на …» и в журнале core.relay. Пыль (решение 222) — по оценке без запаса, как раньше.
//! Ревью t201: С-1 — продажа при итоге ≥ 0 проверяется под ходом очереди (relay_here.rs → sell_here_ok.rs); М-1 — цели
//! не оценены из-за запаса — своя причина со списком; М-3 — run_as(…, false) у причины GasReserve (stuck.rs): без
//! продажи при итоге ≥ 0, прежнее поведение. Звенья пути — в куче (Box::pin, стек: CI t201, core_exec_book_238).
//! Ревью 2 t201 (С-3): правило 1 — продажа здесь под ходом (sell_here_ok::try_sell); под ходом итог < 0 — оценка целей
//! с запасом продолжается с выручкой под ходом, и только если перегон не лучше — продажа здесь по порогу.
//! Настройки: stuck.max_relays, venue.quote_asset, stuck.relay_max_loss_pct (через relay_here.rs); venue.cex_cex,
//! venue.cex_cex_deny (через cex_link.rs); запас — через relay_reserve.rs (новых нет).

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
pub async fn run(core: &Core, deal: Deal) {
    Box::pin(run_as(core, deal, true)).await
}

/// Режим «Перегон»; `profit_first` = false — без продажи здесь при итоге ≥ 0 (решение 417, ревью М-3: причина
/// входа GasReserve — прежнее поведение, иначе быстрый круг «продажа → газ не помещается → режим»). Ошибок нет.
pub async fn run_as(core: &Core, mut deal: Deal, profit_first: bool) {
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
        return Box::pin(super::relay_here::sell_here_as(
            core,
            deal,
            &why,
            profit_first,
        ))
        .await;
    }
    // Решение 220 (часть В): площадка, где лежит монета, на паузе (риск: ошибки подряд, балансы не получены) — вывода
    // с неё не делаем (живой случай Bitget 40938 — каждый перегон с неё отказывал тем же отказом).
    let paused = lk(&core.inner.risk).venue_paused.get(&deal.at).cloned();
    if let Some(p) = paused {
        let why = format!(
            "площадка {} на паузе ({p}) — перегон с неё не делаем",
            deal.at.0
        );
        return Box::pin(super::relay_here::sell_here_as(
            core,
            deal,
            &why,
            profit_first,
        ))
        .await;
    }
    // Решение 230: монета на кошельке DEX на паузе от сверки — перегона с него нет, как с площадки на паузе.
    if let Some(p) = super::wallet_pause::paused_at(core, &deal) {
        let why = format!(
            "{} на паузе ({}) — перегон с него не делаем",
            p.who(),
            p.why
        );
        return Box::pin(super::relay_here::sell_here_as(
            core,
            deal,
            &why,
            profit_first,
        ))
        .await;
    }
    // Решение 227 (дополнение): площадка, где лежит монета, уже отказала в выводе в этой сделке (адресная книга и т. п.)
    // — перегон с неё заведомо откажет: продажа здесь по тем же правилам.
    if super::withdraw_refused::refused(&deal, &deal.at) {
        let why = format!(
            "вывод с {} в этой сделке уже отклонён площадкой — перегон с неё не делаем",
            deal.at.0
        );
        return Box::pin(super::relay_here::sell_here_as(
            core,
            deal,
            &why,
            profit_first,
        ))
        .await;
    }
    // Решение 238 (ревью, п. 2): котировки здесь нет, потому что не задана настройка (NotConfigured, например
    // venue.book_stale_ms площадки), — сделка остановлена с причиной (stuck_quote::halt_not_configured), а не перегон
    // вслепую. CI 85ebf10c (t198): проверка — до чтения факта: остановка не продаёт и не выводит, баланс ей не нужен,
    // а нечитаемый баланс иначе уводил бы сделку мимо остановки решения 238.
    // Седьмое ревью (мелкое 7): возраст котировки — от момента запроса.
    let t0 = tokio::time::Instant::now();
    // Ревью t200 (М-3): момент котировки — продажа ключа после него делает её устаревшей (ниже — котировка заново).
    let at0 = chrono::Utc::now();
    // Решение 415: котировка-решение — под ходом очереди продажи счёта только на время котировки (sell_turn_fresh.rs).
    let pre = Box::pin(super::sell_turn_fresh::once(core, &deal)).await;
    if let Err(e) = &pre
        && super::stuck_quote::not_configured(e)
    {
        return super::stuck_quote::halt_not_configured(core, deal, e).await;
    }
    let qty0 = deal.qty;
    // Решение 413 (ревью t198, Б2): оценка перегона и вывод с биржи — только доли сделки по факту счёта (как перед
    // продажей, sell_fact.rs): чужие монеты на общем счёте не выводятся; None — закрыта или алерт человеку.
    let Some(mut deal) = Box::pin(super::sell_fact::before(core, deal, "перегон")).await
    else {
        return;
    };
    // Решение 413 (пятое ревью t198): остаток зачисления «не присвоено» не решён — с этой биржи не перегоняем (он
    // уехал бы без решения): продажа здесь по тем же правилам, остаток решит тихий момент счёта (credit_rest.rs).
    if super::credit_fact::rest(&deal).is_some() {
        let why = "остаток зачисления на этой бирже не решён — перегон с неё не делаем";
        return Box::pin(super::relay_here::sell_here_as(
            core,
            deal,
            why,
            profit_first,
        ))
        .await;
    }
    let a = deal.direction.asset.clone();
    // Решение 220: котировка — кошельком сделки. Котировка до факта — только если количество то же и с неё прошло
    // меньше venue.book_stale_ms (шестое ревью, С-4: факт мог ждать замок или повторы); иначе — новая.
    let at = At {
        venue: Some(&deal.at.0),
        coin: None,
    };
    let stale = core
        .inner
        .deps
        .settings
        .integer("venue.book_stale_ms", at)
        .ok();
    let fresh = stale.is_some_and(|ms| t0.elapsed() < std::time::Duration::from_millis(ms));
    // Ревью t200 (М-3): пока читался факт, на ключе прошла продажа другой сделки — котировку до неё не берём.
    let sold = super::sell_turn::sold_after(&deal.at, &a, at0);
    let quote = if deal.qty == qty0 && fresh && !sold {
        pre
    } else {
        Box::pin(super::sell_turn_fresh::once(core, &deal)).await
    };
    // Решение 417: средняя котировки здесь — «цена здесь» для доли закрытия разницы в запасе цели (relay_reserve.rs).
    // Dust requires evidence about physical sale proceeds. Lack of a return quote
    // or an uneconomic stablecoin transfer must never write tradable tokens off.
    let physical_here = quote.as_ref().ok().map(|(v, _, _)| *v);
    let physical_avg = quote.as_ref().ok().map(|(_, q, _)| q.avg_price);
    let had_quote = quote.is_ok();
    let quote = Box::pin(super::sell_turn_fresh::settled(core, &deal, quote)).await;
    let return_unknown_here = had_quote && quote.is_err();
    let (mut here, mut here_avg) = match quote {
        Ok((v, q, _)) => (Some(v), Some(q.avg_price)),
        Err(e) if super::stuck_quote::not_configured(&e) => {
            return super::stuck_quote::halt_not_configured(core, deal, &e).await;
        }
        Err(e) => {
            if return_unknown_here {
                deal.note(format!("Продажа здесь: {}", e.message));
                super::ledger::save(core, &deal).await;
            }
            (None, physical_avg)
        }
    };
    // Решение 417 (правило Тиграна 19.11): продать здесь выгодно (итог сделки ≥ 0) — другие площадки не ищем.
    // Ревью t201 (С-1): продажа — с проверкой «выгодно» под ходом очереди (relay_here → sell_here_ok.rs); под ходом
    // итог < 0 — продажа здесь по порогу stuck.relay_max_loss_pct, как раньше. М-3: у GasReserve — без этой ветки.
    // Ревью 2 t201 (С-3, слова Тиграна 19.11: «если у нас выгода больше 0 — в минус не уходим — искать другие
    // площадки даже не нужно; если сделка минусовая — тогда по расчётам понимаем, перегонять выгодно или нет»): под
    // ходом итог < 0 (NotNow) — оценка целей с запасом продолжается с выручкой и средней под ходом; котировки под ходом
    // нет — с котировкой-решением. Перегон не лучше — продажа здесь по порогу (sell_here_as ниже).
    if profit_first && super::relay_reserve::here_first(&deal, here) {
        let why = "перегон не ищем: итог сделки при продаже здесь не ниже нуля (решение 417)";
        match Box::pin(super::sell_here_ok::try_sell(core, deal, why)).await {
            super::sell_here_ok::HereOk::Sold | super::sell_here_ok::HereOk::Gone => return,
            super::sell_here_ok::HereOk::NotNow(d, q) => {
                deal = *d;
                if let Some((v, avg)) = q {
                    here = Some(v);
                    here_avg = Some(avg);
                }
            }
        }
    }
    let mut best: Option<(VenueId, super::relay_reserve::Est)> = None;
    // Решение 222: лучшая оценка без запаса — для признака пыли, как до решения 417.
    let mut raw_best: Option<Decimal> = None;
    // Ревью t201 (М-1): цели, не оценённые из-за запаса (σ / настройки), — «площадка: причина».
    let mut no_reserve: Vec<String> = Vec::new();
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
        let r = super::relay_value::value_parts(core, &deal.at, &to, &a, deal.qty, acc.as_ref());
        match Box::pin(r).await.map(|p| (p.net, p)) {
            Ok((v, _)) if v <= Decimal::ZERO => {
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
            Ok((v, p)) => {
                raw_best = Some(raw_best.map_or(v, |b| b.max(v)));
                let v = if super::manual_price::needs_individual_return(core, &deal) {
                    match Box::pin(super::manual_price::individual_value(
                        core, &deal, &to, v, p.cash,
                    ))
                    .await
                    {
                        Ok(net) => net,
                        Err(why) => {
                            deal.note(format!("Перегон на {}: {why}", to.0));
                            continue;
                        }
                    }
                } else {
                    v
                };
                // Решение 417: оценка цели — с запасом на движение цены за перевод, как у плана входа.
                match super::relay_reserve::params_at(core, &deal.at, &to, &a, chrono::Utc::now()) {
                    Ok(rp) => {
                        let e = super::relay_reserve::estimate(v, p.proceeds, p.avg, here_avg, rp);
                        let t = format!(
                            "{}: перегон {} {} на {}: {}; продажа здесь {}",
                            deal.id,
                            deal.qty,
                            a.ticker,
                            to.0,
                            super::relay_reserve::text(&e),
                            here.map_or(
                                "— нет котировки".to_string(),
                                crate::notify::human::signed_usd
                            )
                        );
                        core.inner
                            .deps
                            .log(Level::Info, "core.relay", Some(&to.0), t, None)
                            .await;
                        if best.as_ref().is_none_or(|(_, b)| e.net() > b.net()) {
                            best = Some((to, e));
                        }
                    }
                    Err(err) => {
                        let t = format!(
                            "{}: перегон на {} не оценён: запас на движение цены за перевод не посчитан: {err}",
                            deal.id, to.0
                        );
                        no_reserve.push(format!("{}: {err}", to.0));
                        core.inner
                            .deps
                            .log(Level::Info, "core.relay", Some(&to.0), t, None)
                            .await
                    }
                }
            }
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
    if super::dust::relay_dust(raw_best, tiny, here.max(physical_here)) {
        let why =
            "остаток — пыль: перегон не окупается, продажа здесь не даёт выручки (решение 222)";
        super::dust::write_off(core, &mut deal, why).await;
        return super::ledger::finish(core, &mut deal, Step::Closed, why).await;
    }
    // Решение 417: перегон — только если оценка цели с запасом больше выручки здесь (чистыми).
    // When local settlement cannot be priced, only a target beating even the local
    // cash before any return fees is provably better. Otherwise the normal local
    // decision reports the missing quote, retaining the operator's manual sale.
    let comparison_here = if return_unknown_here {
        physical_here
    } else {
        here
    };
    let (to, e) = match best {
        Some((to, e)) if super::relay_reserve::better(comparison_here, &e) => (to, e),
        Some((to, e)) => {
            let why = format!(
                "перегон не выгоднее продажи здесь: лучшая цель {} — {} (решение 417)",
                to.0,
                super::relay_reserve::text(&e)
            );
            return Box::pin(super::relay_here::sell_here_as(
                core,
                deal,
                &why,
                profit_first,
            ))
            .await;
        }
        // Ревью t201 (М-1): целей с оценкой нет, потому что запас не посчитан (σ / настройки), — своя причина со
        // списком, а не «перегон не выгоднее».
        None if !no_reserve.is_empty() => {
            let why = format!(
                "цели перегона не оценены: нет запаса на движение цены за перевод (σ / настройки) — {} (решение 417)",
                no_reserve.join("; ")
            );
            return Box::pin(super::relay_here::sell_here_as(
                core,
                deal,
                &why,
                profit_first,
            ))
            .await;
        }
        None => {
            let why = "перегон не выгоднее продажи здесь";
            return Box::pin(super::relay_here::sell_here_as(
                core,
                deal,
                why,
                profit_first,
            ))
            .await;
        }
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
    // Решение 413 (повторное ревью, С-1): получатель перегона — для распределения долей на его счёте.
    deal.transit_to = Some(to.clone());
    // Решение 417: в хронологии — расчёт запаса (ожидается с запасом, без запаса, запас и его части).
    deal.note(format!(
        "Перегон на {} ({})",
        crate::notify::human::venue(&to.0),
        super::relay_reserve::text(&e)
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
    // Решение 413 (третье ревью, М-2): вывода не было (исход известен) — сделка не в пути.
    if matches!(&res, Err(e) if !e.outcome_unknown()) {
        deal.transit_to = None;
    }
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
