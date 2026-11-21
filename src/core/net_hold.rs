//! Файл: core/net_hold.rs
//! Что делает: решение 489 — сеть выключенной площадки DEX держится, пока по ней идут сделки (по образцу решения 196 для
//! потока, feed_hold.rs). Держатель сети — сделка, которая держит поток площадки (feed_hold::holds: открыта или
//! остановлена с монетами, площадка — покупка, продажа или место монет), ИЛИ закрытая сделка, чей возврат USDT ещё идёт:
//! часть «идёт» / «ждёт повтора» (deal_return_cost::in_progress — им же откладывается deal.closed, решение 236), где
//! площадка — откуда или куда перегон (usdt_return.rs relay_try: usdt(core, from) — USDT сети площадки DEX,
//! registry_chains::usdt_of — есть, только пока группа сети в игре), ИЛИ сделка группы, чья выручка ждёт общего
//! возврата группы с этой площадки (DealGroup.wait — у любой сделки группы: носитель общего возврата — первая, если её
//! часть в пуле, иначе первая отложенная, coin_group_flush.rs). Поток (подписка) по-прежнему держит только holds.
//! Держатели ищутся среди незавершённых сделок (unsettled.rs: keep — надмножество держателя), не по всей истории.
//! Переходы (Inner.net_held — площадки, чью сеть сейчас держат сделки; отмечает и chain_play.rs): сохранение сделки
//! (ledger.rs save → on_saved) — захват: площадка этой сделки — DEX, не включена, сделка держит её сеть, а в памяти её
//! нет (повторный захват: «Перегнать» по «не вернулись», «Вернуть боту») — отметка и толчок; отпускание: площадка из
//! памяти больше не держится — убрать и толчок. Толчок — перечитывание справочника сетей бирж без потери (net_kick.rs)
//! и пересборка. Повторные сохранения без перехода толчков не дают.
//! Кто вызывает: core/chain_play.rs (holds_net_any, mark), core/ledger.rs (on_saved).
//! От чего зависит: feed_hold.rs (holds), unsettled.rs, usdt_return_parts.rs, usdt_return_state.rs, deal.rs,
//! deal_group.rs, daily_venue.rs, registry_sync.rs (venue.enabled), net_kick.rs, engine.rs.
//! Настройки: venue.enabled (через registry_sync::enabled).

use crate::core::types::VenueId;

use super::deal::Deal;
use super::engine::{Core, lk};
use super::usdt_return_state::UsdtReturn;

/// Что делает: держит ли сделка сеть площадки (шапка). Принимает: сделку, площадку. Возвращает: да/нет. Ошибок нет.
pub fn holds_net(d: &Deal, v: &VenueId) -> bool {
    super::feed_hold::holds(d, v) || returning(d, v) || group_wait(d, v)
}

/// Возврат USDT сделки идёт (часть «идёт» или «ждёт повтора») с площадки `v` или на неё. Ошибок нет.
fn returning(d: &Deal, v: &VenueId) -> bool {
    super::usdt_return_parts::all(d)
        .into_iter()
        .filter_map(way)
        .any(|(from, to)| from == v || to == v)
}

/// Откуда и куда части возврата, которая ещё идёт («идёт», «ждёт повтора»); иначе None. Ошибок нет.
fn way(p: &UsdtReturn) -> Option<(&VenueId, &VenueId)> {
    match p {
        UsdtReturn::Pending { from, to, .. } => Some((from, to)),
        UsdtReturn::Retrying { from, to, .. } => Some((from, to)),
        _ => None,
    }
}

/// Выручка сделки группы ждёт общего возврата группы с площадки `v` (DealGroup.wait, любая сделка группы). Ошибок нет.
fn group_wait(d: &Deal, v: &VenueId) -> bool {
    let from = d.group.as_ref().and_then(|g| g.wait.as_ref()).map(|w| &w.0);
    from == Some(v)
}

/// Площадки сделки, чью сеть она может держать: покупка, продажа, место монет, откуда/куда частей возврата, откуда
/// ожидание общего возврата группы. Ошибок нет.
fn venues_of(d: &Deal) -> Vec<VenueId> {
    let mut out = vec![
        d.direction.buy_on.clone(),
        d.direction.sell_on.clone(),
        d.at.clone(),
    ];
    let parts = super::usdt_return_parts::all(d);
    for (from, to) in parts.into_iter().filter_map(way) {
        out.push(from.clone());
        out.push(to.clone());
    }
    if let Some((from, _)) = d.group.as_ref().and_then(|g| g.wait.as_ref()) {
        out.push(from.clone());
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out.dedup();
    out
}

/// Что делает: держит ли сеть площадки хоть одна незавершённая сделка (unsettled.rs). Принимает: ядро, площадку.
/// Возвращает: да/нет. Ошибок нет. Замки: unsettled (копия), затем deals — не вложенно.
pub fn holds_net_any(core: &Core, v: &VenueId) -> bool {
    let ids = super::unsettled::ids(core);
    let deals = lk(&core.inner.deals);
    ids.iter()
        .any(|id| deals.get(id).is_some_and(|d| holds_net(d, v)))
}

/// Что делает: запоминает, что сеть выключенной площадки сейчас держат сделки. Принимает: ядро, площадку.
/// Возвращает: новая ли отметка. Ошибок нет. Замок: net_held (коротко).
pub fn mark(core: &Core, v: &VenueId) -> bool {
    lk(&core.inner.net_held).insert(v.clone())
}

/// Что делает: сделка `d` сохранена — захват (её площадка — DEX, не включена, сделка держит её сеть, отметки нет) и
/// отпускание (площадки из памяти: включили — забыть без толчка; держателей нет — убрать); был переход — толчок
/// перечитывания справочника сетей бирж (net_kick.rs) и пересборки. Принимает: ядро, сделку. Возвращает: был ли толчок.
/// Ошибок нет. Замки: net_held, unsettled, deals — по очереди, не вложенно.
pub fn on_saved(core: &Core, d: &Deal) -> bool {
    let mut kick = false;
    for v in venues_of(d) {
        let dex = super::daily_venue::is_dex(core, &v.0);
        if dex && !super::registry_sync::enabled(core, &v) && holds_net(d, &v) {
            kick |= mark(core, &v);
        }
    }
    let held: Vec<VenueId> = lk(&core.inner.net_held).iter().cloned().collect();
    for v in held {
        let on = super::registry_sync::enabled(core, &v);
        if !on && holds_net_any(core, &v) {
            continue;
        }
        lk(&core.inner.net_held).remove(&v);
        kick |= !on;
    }
    if kick {
        super::net_kick::kick(core);
        core.request_rebuild();
    }
    kick
}
