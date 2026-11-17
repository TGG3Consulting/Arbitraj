//! Файл: core/coin_group.rs
//! Что делает: решение 394 (t179) — общий возврат USDT группы докупки (записка 269, пп. 10, 11, 13; ревью 2 t179 —
//! Б1, Б2, Н1; ревью 3 — Б1, Н1, Н3, п. 13). Только у сделок с группой (Deal.group; без группы — как раньше, сюда не
//! попадают).
//! - Закрытие обычной продажей на площадке плана одной частью — часть не перегоняется сразу: Deal.group.wait, запись
//!   сделки сохраняется (hold_parts, вызывает prepare в usdt_return_deal.rs). Иначе (перегон, части на нескольких
//!   площадках, продажа вручную) или сделка уже вышла из группы (left, п. 13: была вне пути) — свой возврат.
//! - После сохранения закрытия (ledger::finish → after_close; любой шаг: Closed, Halted, TakenOver) — под замком
//!   `groups`: часть — в пул группы с итогом сделки из её записи; сделка — среди закрывшихся. Последняя (нет идущих
//!   сделок группы на шаге Transfer / Selling, кроме закрывшихся и вышедших из группы, и нет вложенных до учёта покупки
//!   — `pending`, Б2) — пул уходит ОДНИМ общим возвратом (coin_group_flush.rs). Ушедшая с пути последняя идущая (Stuck
//!   и др., on_saved) — тоже.
//! - Вложенная входит в `pending` в самом admit, под замком монеты целиком (ревью 3, Н1: порядок замков active →
//!   groups; окна между admit и join больше нет).
//! - Пул после общего возврата остаётся с признаком `flushed`, пока у группы есть идущие записи: позже закрывшаяся
//!   сделка группы — «была частью группы», свой возврат (п. 13); новые докупки в такую группу не входят (ревью 3, Н3 —
//!   coin_ride.rs: новая группа, фиксированная часть возврата — в её плане).
//!
//! Отложенные части живут в записях сделок (Deal.group.wait, база): перезапуск — «не вернулись» с точной суммой,
//! решает человек (ledger_load.rs).
//! Кто вызывает: usdt_return_deal.rs (hold_parts), ledger.rs (finish → after_close; save → on_saved), coin_ride.rs
//! (admit → join, flushed), executor.rs через ride_guard.rs (joined, dropped, released).
//! От чего зависит: coin_key.rs, coin_ride.rs (идущие), deal.rs, coin_group_flush.rs (общий возврат), engine.rs.
//! Настройки: нет (пределы вывода — coin_group_flush.rs; включатель — coin_ride.rs).

use std::collections::HashSet;

use rust_decimal::Decimal;

use crate::core::types::VenueId;

use super::coin_key::CoinKey;
use super::deal::{Deal, Step};
use super::engine::{Core, lk};

pub use super::coin_group_flush::pieces;

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
/// (Deal.group.wait, перегона сейчас нет); иначе сделка вышла из группы (left) — свой возврат. Сделка уже вне группы
/// (п. 13: была вне пути — coin_group_left.rs) — свой возврат без новой строки. Принимает: сделку, её части (откуда,
/// сумма). Возвращает: части, которые перегнать сейчас. Ошибок нет.
pub fn hold_parts(deal: &mut Deal, parts: Vec<(VenueId, Decimal)>) -> Vec<(VenueId, Decimal)> {
    let Some(g) = deal.group.as_ref().map(|x| x.id.clone()) else {
        return parts;
    };
    if deal.group.as_ref().is_some_and(|x| x.left) {
        return parts;
    }
    if normal(deal) && parts.len() == 1 {
        let (from, amount) = parts[0].clone();
        if let Some(x) = deal.group.as_mut() {
            x.wait = Some((from.clone(), amount));
        }
        deal.note(format!(
            "Возврат USDT {amount} с {}: 0 сейчас — общий возврат группы {g} после последней её сделки (решение 394)",
            from.0
        ));
        return vec![];
    }
    if let Some(x) = deal.group.as_mut() {
        x.left = true;
    }
    deal.note(format!(
        "Была частью группы докупки {g}: закрыта не обычной продажей на площадке плана — свой возврат USDT (решение 394)"
    ));
    parts
}

/// Сколько записей группы `g` монеты ещё в идущих и не закрылись (не в `closing`). `path` — только идущие на пути
/// (Transfer / Selling) и уже закрытые, но ещё не прошедшие after_close (Closed не из `closing`: их часть ещё придёт в
/// пул — без этого при одновременном закрытии первая отдавала бы свою часть одна, а вторая — свою, общий возврат
/// делился); вышедшие из группы (left, п. 13) не в счёт — их часть в пул не придёт. Вызывается под замком `groups`
/// (порядок groups → rides → deals). Ошибок нет.
fn running(core: &Core, key: &CoinKey, g: &str, closing: &HashSet<String>, path: bool) -> usize {
    let rides = lk(&core.inner.rides);
    let deals = lk(&core.inner.deals);
    rides.get(key).map_or(0, |v| {
        v.iter()
            .filter(|r| r.group.as_deref() == Some(g) && !closing.contains(&r.deal_id))
            .filter(|r| {
                !path
                    || deals.get(&r.deal_id).is_some_and(|x| {
                        matches!(x.step, Step::Transfer | Step::Selling | Step::Closed)
                            && !x.group.as_ref().is_some_and(|y| y.left)
                    })
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
        Box::pin(super::coin_group_flush::flush(core, &g, parts, was)).await;
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

/// Под замком `groups` правка пула (`f`), затем «последняя ли» — да: общий возврат отдельной задачей (нет рантайма —
/// задача не ставится: вызов из Drop при остановке процесса, ride_guard.rs; отложенные части — в записях сделок,
/// перезапуск ставит «не вернулись»). Ошибок нет.
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
    if let Some((parts, was)) = taken
        && let Ok(h) = tokio::runtime::Handle::try_current()
    {
        let (c, gid) = (core.clone(), g.to_string());
        h.spawn(
            async move { Box::pin(super::coin_group_flush::flush(&c, &gid, parts, was)).await },
        );
    }
}

/// Что делает: Б2 — вложенная сделка группы вошла (admit, до покупки; вызывается под замком `active`, ревью 3, Н1):
/// общий возврат группы ждёт её. Принимает: ядро, группу. Ошибок нет.
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

/// Что делает: ревью 3, Н3 — общий возврат группы `g` уже ушёл (пул с признаком flushed): новые докупки в неё не входят.
/// Пула нет (группа ещё ничего не закрыла или убрана) — false. Принимает: ядро, группу. Ошибок нет.
pub fn flushed(core: &Core, g: &str) -> bool {
    lk(&core.inner.groups).get(g).is_some_and(|p| p.flushed)
}
