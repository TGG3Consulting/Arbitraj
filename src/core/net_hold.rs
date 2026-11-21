//! Файл: core/net_hold.rs
//! Что делает: решение 489 — сеть выключенной площадки DEX держится, пока по ней идут сделки (по образцу решения 196 для
//! потока, feed_hold.rs). Держатель сети — сделка, которая держит поток площадки (feed_hold::holds: открыта или
//! остановлена с монетами, площадка — покупка, продажа или место монет), ИЛИ закрытая сделка, чей возврат USDT ещё идёт:
//! часть «идёт» / «ждёт повтора» (deal_return_cost::in_progress — им же откладывается deal.closed, решение 236), где
//! площадка — откуда или куда перегон (usdt_return.rs relay_try: usdt(core, from) — USDT сети площадки DEX,
//! registry_chains::usdt_of — есть, только пока группа сети в игре), или первая сделка группы, чья выручка ждёт общего
//! возврата группы (DealGroup.wait — откуда, решение 394). Поток (подписка) по-прежнему держит только feed_hold::holds.
//! Отпускание: ядро помнит площадки, чью сеть держат сделки (Inner.net_held — отмечает chain_play.rs, когда так решено
//! при привязке записей и сборке групп); сохранение сделки (ledger.rs save) проверяет только их — сеть отпущена
//! (сделок-держателей нет, площадка не включена) → один толчок перечитывания справочника сетей бирж и пересборки.
//! Повторные сохранения закрытой сделки (возврат USDT) толчков не дают: площадки в памяти уже нет.
//! Кто вызывает: core/chain_play.rs (holds_net_any, mark), core/ledger.rs (on_saved).
//! От чего зависит: feed_hold.rs (holds), usdt_return_parts.rs, usdt_return_state.rs, deal.rs, deal_group.rs,
//! registry_sync.rs (venue.enabled), engine.rs.
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
    for p in super::usdt_return_parts::all(d) {
        let (from, to) = match p {
            UsdtReturn::Pending { from, to, .. } => (from, to),
            UsdtReturn::Retrying { from, to, .. } => (from, to),
            _ => continue,
        };
        if from == v || to == v {
            return true;
        }
    }
    false
}

/// Первая сделка группы, чья выручка ждёт общего возврата группы с площадки `v` (DealGroup.wait). Ошибок нет.
fn group_wait(d: &Deal, v: &VenueId) -> bool {
    let Some(g) = d.group.as_ref() else {
        return false;
    };
    let from = g.wait.as_ref().map(|w| &w.0);
    g.id == d.id && from == Some(v)
}

/// Что делает: держит ли сеть площадки хоть одна сделка. Принимает: ядро, площадку. Возвращает: да/нет. Ошибок нет.
/// Замок: deals (коротко, не вложенно).
pub fn holds_net_any(core: &Core, v: &VenueId) -> bool {
    lk(&core.inner.deals).values().any(|d| holds_net(d, v))
}

/// Что делает: запоминает, что сеть выключенной площадки сейчас держат сделки (для одного толчка на отпускании).
/// Принимает: ядро, площадку. Ошибок нет. Замок: net_held (коротко).
pub fn mark(core: &Core, v: &VenueId) {
    lk(&core.inner.net_held).insert(v.clone());
}

/// Что делает: сделка сохранена — сети площадок из памяти, которые больше не держатся (площадку включили — забыть без
/// толчка; сделок-держателей нет — отпустить), убираются; отпущена хоть одна — толчок перечитывания справочника сетей
/// бирж и пересборки. Принимает: ядро. Возвращает: был ли толчок. Ошибок нет. Замки: net_held и deals — по очереди.
pub fn on_saved(core: &Core) -> bool {
    let held: Vec<VenueId> = lk(&core.inner.net_held).iter().cloned().collect();
    let mut kick = false;
    for v in held {
        let on = super::registry_sync::enabled(core, &v);
        if !on && holds_net_any(core, &v) {
            continue;
        }
        lk(&core.inner.net_held).remove(&v);
        kick |= !on;
    }
    if kick {
        core.inner.kick_networks.notify_waiters();
        core.request_rebuild();
    }
    kick
}
