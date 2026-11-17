//! Файл: core/coin_group.rs
//! Что делает: решение 390 (t179) — общий возврат USDT группы докупки (записка 269, пп. 10, 11, 13; ревью 2 t179 —
//! Б1, Б2, Н1, Н2). Только у сделок с группой (Deal.group; без группы — как раньше, сюда не попадают).
//! - Закрытие обычной продажей на площадке плана одной частью — часть не перегоняется сразу: Deal.group.wait, запись
//!   сделки сохраняется (hold_parts, вызывает prepare в usdt_return_deal.rs). Иначе (перегон, части на нескольких
//!   площадках, продажа вручную) — сделка вышла из группы (п. 13): свой возврат.
//! - После сохранения закрытия (ledger::finish → after_close; любой шаг: Closed, Halted, TakenOver) — под замком
//!   `groups`: часть — в пул группы с итогом сделки из её записи; сделка — среди закрывшихся. Последняя (нет идущих
//!   сделок группы на шаге Transfer / Selling, кроме закрывшихся, и нет вложенных до учёта покупки — `pending`, Б2) —
//!   пул уходит ОДНИМ общим возвратом (flush). Ушедшая с пути последняя идущая (Stuck и др., on_saved) — тоже flush.
//! - flush (Н1 — один порядок): носитель — первая сделка группы, если её часть в пуле, иначе первая отложенная; под
//!   замком учёта у всех частей снимается wait, у остальных — «общий возврат в сделке …», у носителя — итог группы
//!   (сумма итогов из пула); перегоны готовятся у носителя (состояние «идёт»); ВСЕ записи сохраняются; только потом
//!   перегон стартует. Падение до сохранения — у сделок wait → при запуске «не вернулись» по своим суммам (ledger_load);
//!   после сохранения — «не вернулись» у носителя на общую сумму, у остальных возврата нет (двойного вывода нет;
//!   «Перегнать» по сделке с общим возвратом — отказ, manual_usdt.rs retry_block).
//! - Минимум и максимум вывода USDT у площадки возврата (Н2): сумма выше максимума — несколько выводов не больше
//!   максимума (поровну); ниже минимума — одним выводом, как у одиночной сделки.
//! - Пул после flush остаётся с признаком `flushed`, пока у группы есть идущие записи: позже закрывшаяся (застрявшая)
//!   сделка группы — «была частью группы», свой возврат (п. 13).
//!
//! Отложенные части живут в записях сделок (Deal.group.wait, база): перезапуск — «не вернулись» с точной суммой,
//! решает человек (ledger_load.rs).
//! Кто вызывает: usdt_return_deal.rs (hold_parts), ledger.rs (finish → after_close; save → on_saved), executor.rs (join,
//! joined, dropped, released).
//! От чего зависит: coin_key.rs, coin_ride.rs (идущие), deal.rs, deal_group.rs, ledger.rs, usdt_return_deal.rs,
//! dirctx_venue.rs (сеть USDT площадки), engine.rs, settings.
//! Настройки: venue.quote_asset, venue.bsc_network (сеть возврата — пределы вывода); включатель — coin_ride.rs.

use std::collections::HashSet;

use rust_decimal::Decimal;

use crate::core::types::VenueId;
use crate::settings::read::At;

use super::coin_key::CoinKey;
use super::deal::{Deal, Step};
use super::deal_group::GroupSum;
use super::engine::{Core, lk};

/// Отложенная часть общего возврата: сделка, откуда, сумма, итог сделки.
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub deal: String,
    pub from: VenueId,
    pub amount: Decimal,
    pub result: Option<Decimal>,
}

/// Состояние группы.
#[derive(Debug, Default)]
pub struct Pool {
    closing: HashSet<String>,
    parts: Vec<Part>,
    /// Вложенные сделки до учёта покупки (admit → board), Б2.
    pending: usize,
    /// Общий возврат группы уже ушёл.
    flushed: bool,
}

/// Обычное закрытие сделки группы: продано ботом на площадке плана, без перегонов, без исключённого пути и «Продать и
/// перегнать». Ошибок нет.
fn normal(deal: &Deal) -> bool {
    deal.at == deal.direction.sell_on
        && deal.relays == 0
        && !deal.sells.is_empty()
        && deal.usdt_return_skip.is_none()
        && !deal.return_usdt
}

/// Что делает: части возврата закрытой сделки группы — у обычного закрытия одной частью часть ждёт общего возврата
/// (Deal.group.wait, перегона сейчас нет); иначе сделка вышла из группы (left) — свой возврат. Принимает: сделку, её
/// части (откуда, сумма). Возвращает: части, которые перегнать сейчас. Ошибок нет.
pub fn hold_parts(deal: &mut Deal, parts: Vec<(VenueId, Decimal)>) -> Vec<(VenueId, Decimal)> {
    let Some(g) = deal.group.as_ref().map(|x| x.id.clone()) else {
        return parts;
    };
    if normal(deal) && parts.len() == 1 {
        let (from, amount) = parts[0].clone();
        if let Some(x) = deal.group.as_mut() {
            x.wait = Some((from.clone(), amount));
        }
        deal.note(format!(
            "Возврат USDT {amount} с {}: 0 сейчас — общий возврат группы {g} после последней её сделки (решение 390)",
            from.0
        ));
        return vec![];
    }
    if let Some(x) = deal.group.as_mut() {
        x.left = true;
    }
    deal.note(format!(
        "Была частью группы докупки {g}: закрыта не обычной продажей на площадке плана — свой возврат USDT (решение 390)"
    ));
    parts
}

/// Сколько записей группы `g` монеты ещё в идущих и не закрылись (не в `closing`). `path` — только идущие на пути
/// (Transfer / Selling) и уже закрытые, но ещё не прошедшие after_close (Closed не из `closing`: их часть ещё придёт в
/// пул — без этого при одновременном закрытии первая отдавала бы свою часть одна, а вторая — свою, общий возврат
/// делился). Вызывается под замком `groups` (порядок groups → rides → deals). Ошибок нет.
fn running(core: &Core, key: &CoinKey, g: &str, closing: &HashSet<String>, path: bool) -> usize {
    let rides = lk(&core.inner.rides);
    let deals = lk(&core.inner.deals);
    rides.get(key).map_or(0, |v| {
        v.iter()
            .filter(|r| r.group.as_deref() == Some(g) && !closing.contains(&r.deal_id))
            .filter(|r| {
                !path
                    || matches!(
                        deals.get(&r.deal_id).map(|x| x.step),
                        Some(Step::Transfer | Step::Selling | Step::Closed)
                    )
            })
            .count()
    })
}

/// Под замком: последняя ли — нет идущих на пути и вложенных до покупки; да — части пула забираются (пул остаётся с
/// признаком flushed, пока у группы есть записи в идущих). Возвращает: части для общего возврата и был ли flush раньше.
fn take_if_last(core: &Core, key: &CoinKey, g: &str, pool: &mut Pool) -> Option<(Vec<Part>, bool)> {
    if pool.pending > 0 || running(core, key, g, &pool.closing, true) > 0 || pool.parts.is_empty() {
        return None;
    }
    let was = pool.flushed;
    pool.flushed = true;
    Some((std::mem::take(&mut pool.parts), was))
}

/// Убрать пул, если у группы не осталось ни одной записи в идущих (и закрывшихся тоже — их список `closing` нужен, пока
/// их записи не сняты, released) и вложенных до покупки (вызывается под замком). Ошибок нет.
fn tidy(core: &Core, key: &CoinKey, g: &str, groups: &mut std::collections::HashMap<String, Pool>) {
    if let Some(p) = groups.get(g)
        && p.parts.is_empty()
        && p.pending == 0
        && running(core, key, g, &HashSet::new(), false) == 0
    {
        groups.remove(g);
    }
}

/// Что делает: сделка группы закрыта и сохранена (ledger::finish, любой конечный шаг) — её отложенная часть в пул,
/// она среди закрывшихся; последняя — общий возврат (flush). Запись сделки после общего возврата перечитывается.
/// Принимает: ядро, сделку. Ошибок нет.
pub async fn after_close(core: &Core, deal: &mut Deal) {
    let Some(g) = deal.group.as_ref().map(|x| x.id.clone()) else {
        return;
    };
    let key = CoinKey::of(&deal.direction.asset);
    let wait = deal.group.as_ref().and_then(|x| x.wait.clone());
    let taken = {
        let mut groups = lk(&core.inner.groups);
        let pool = groups.entry(g.clone()).or_default();
        pool.closing.insert(deal.id.clone());
        if let Some((from, amount)) = wait {
            pool.parts.push(Part {
                deal: deal.id.clone(),
                from,
                amount,
                result: deal.result_usd,
            });
        }
        let t = take_if_last(core, &key, &g, pool);
        tidy(core, &key, &g, &mut groups);
        t
    };
    if let Some((parts, was)) = taken {
        Box::pin(flush(core, &g, parts, was)).await;
        if let Some(x) = lk(&core.inner.deals).get(&deal.id).cloned() {
            *deal = x;
        }
    }
}

/// Что делает: сделка группы сохранена на шаге вне пути (застряла, ждёт человека, остановлена, передана человеку) —
/// если она была последней идущей, отложенные части уходят общим возвратом сразу (п. 13). Принимает: ядро, сделку.
/// Ошибок нет (общий возврат — отдельной задачей).
pub fn on_saved(core: &Core, deal: &Deal) {
    let Some(g) = deal.group.as_ref().map(|x| x.id.clone()) else {
        return;
    };
    if matches!(
        deal.step,
        Step::Buying | Step::Transfer | Step::Selling | Step::Closed
    ) {
        return;
    }
    spawn_if_last(core, &CoinKey::of(&deal.direction.asset), &g, |_| {});
}

/// Под замком `groups` правка пула (`f`), затем «последняя ли» — да: общий возврат отдельной задачей. Ошибок нет.
fn spawn_if_last(core: &Core, key: &CoinKey, g: &str, f: impl FnOnce(&mut Pool)) {
    let taken = {
        let mut groups = lk(&core.inner.groups);
        let Some(pool) = groups.get_mut(g) else {
            return;
        };
        f(pool);
        let t = take_if_last(core, key, g, pool);
        tidy(core, key, g, &mut groups);
        t
    };
    if let Some((parts, was)) = taken {
        let (c, g) = (core.clone(), g.to_string());
        tokio::spawn(async move { Box::pin(flush(&c, &g, parts, was)).await });
    }
}

/// Что делает: Б2 — вложенная сделка группы вошла (admit, до покупки): общий возврат группы ждёт её. Принимает: ядро,
/// группу. Ошибок нет.
pub fn join(core: &Core, g: &str) {
    lk(&core.inner.groups)
        .entry(g.to_string())
        .or_default()
        .pending += 1;
}

/// Что делает: вложенная сделка учла покупку и в идущих (board) — дальше её считают идущие. Принимает: ядро, группу.
/// Ошибок нет.
pub fn joined(core: &Core, g: &str) {
    if let Some(p) = lk(&core.inner.groups).get_mut(g) {
        p.pending = p.pending.saturating_sub(1);
    }
}

/// Что делает: вложенная сделка не состоялась (задача кончилась до board: отказ предпроверки, покупки) — выходит из
/// группы; если других нет — общий возврат сразу. Принимает: ядро, ключ монеты, группу. Ошибок нет.
pub fn dropped(core: &Core, key: &CoinKey, g: &str) {
    spawn_if_last(core, key, g, |p| p.pending = p.pending.saturating_sub(1));
}

/// Что делает: запись сделки группы снята из идущих (задача сделки кончилась) — проверка «последняя ли» и уборка пула.
/// Принимает: ядро, ключ монеты, группу. Ошибок нет.
pub fn released(core: &Core, key: &CoinKey, g: &str) {
    spawn_if_last(core, key, g, |_| {});
}

/// Пределы вывода USDT площадки возврата `v` (сеть venue.bsc_network, монета venue.quote_asset; справочник сетей
/// ядра). Возвращает: (минимум, максимум); нет данных (DEX, справочника нет) — (None, None). Ошибок нет.
fn limits(core: &Core, v: &VenueId) -> (Option<Decimal>, Option<Decimal>) {
    let s = &core.inner.deps.settings;
    let at = At {
        venue: Some(&v.0),
        coin: None,
    };
    let (Ok(q), Ok(net)) = (
        s.text("venue.quote_asset", at),
        s.text("venue.bsc_network", at),
    ) else {
        return (None, None);
    };
    match super::dirctx_venue::quote_network(core, v, &q, &net) {
        Some(i) => (i.withdraw_min, i.withdraw_max.filter(|m| !m.is_zero())),
        None => (None, None),
    }
}

/// Сумма общего возврата частями не больше максимума вывода (поровну; последняя — остаток). Ошибок нет.
pub fn pieces(from: &VenueId, total: Decimal, max: Option<Decimal>) -> Vec<(VenueId, Decimal)> {
    let Some(max) = max.filter(|m| total > *m && *m > Decimal::ZERO) else {
        return vec![(from.clone(), total)];
    };
    let n = (total / max).ceil();
    let each = (total / n).round_dp_with_strategy(8, rust_decimal::RoundingStrategy::ToZero);
    let k = rust_decimal::prelude::ToPrimitive::to_usize(&n)
        .unwrap_or(1)
        .max(1);
    let mut out = vec![(from.clone(), each); k - 1];
    out.push((from.clone(), total - each * Decimal::from(k - 1)));
    out
}

/// Общий возврат частей одним перегоном (Н1 — порядок в шапке). Принимает: ядро, группу, части, был ли flush раньше
/// (тогда это сделка, закрывшаяся после общего возврата, — «была частью группы», свой возврат). Ошибок нет.
async fn flush(core: &Core, g: &str, parts: Vec<Part>, was: bool) {
    let Some(first) = parts.first().cloned() else {
        return;
    };
    let bearer = parts.iter().find(|p| p.deal == g).cloned().unwrap_or(first);
    let others: Vec<&Part> = parts.iter().filter(|p| p.deal != bearer.deal).collect();
    let total: Decimal = parts.iter().map(|p| p.amount).sum();
    let (copies, lead) = {
        let mut deals = lk(&core.inner.deals);
        let mut copies = vec![];
        for p in &others {
            if let Some(d) = deals.get_mut(&p.deal) {
                if let Some(x) = d.group.as_mut() {
                    x.wait = None;
                    x.shared_in = Some(bearer.deal.clone());
                }
                d.note(format!(
                    "Возврат USDT: 0 — общий возврат группы в сделке {} (решение 390)",
                    bearer.deal
                ));
                copies.push(d.clone());
            }
        }
        let lead = deals.get_mut(&bearer.deal).map(|d| {
            if let Some(x) = d.group.as_mut() {
                x.wait = None;
                x.shared_in = None;
                x.left = was;
                if !others.is_empty() {
                    x.sum = Some(GroupSum {
                        deals: others.iter().map(|p| p.deal.clone()).collect(),
                        others: others.iter().filter_map(|p| p.result).sum(),
                        usdt: others.iter().map(|p| p.amount).sum(),
                    });
                }
            }
            d.clone()
        });
        (copies, lead)
    };
    let Some(mut lead) = lead else {
        return;
    };
    let why = if was {
        format!("была частью группы {g}, общий возврат группы ушёл раньше — свой возврат")
    } else {
        format!(
            "общий возврат USDT группы {g}: {total} (сделки {}) — затраты возврата один раз, в этой сделке",
            parts
                .iter()
                .map(|p| p.deal.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    lead.note(format!("{why} (решение 390)"));
    let (_, max) = limits(core, &bearer.from);
    let jobs = Box::pin(super::usdt_return_deal::start_parts(
        core,
        &mut lead,
        pieces(&bearer.from, total, max),
    ))
    .await;
    // Перегон не начат (страховка «не вернулись») — отложенное уведомление носителя уходит сразу.
    let notice = if jobs.is_empty() {
        super::deal_closed::take_ready(&mut lead)
    } else {
        None
    };
    for d in &copies {
        Box::pin(super::ledger::save(core, d)).await;
    }
    Box::pin(super::ledger::save(core, &lead)).await;
    if let Some(w) = notice {
        Box::pin(super::deal_closed::send(core, &lead, &w)).await;
    }
    for j in jobs {
        j.start(core);
    }
}
