//! Файл: core/cex_link.rs
//! Что делает: решение 249 (Тигран 10.11) — можно ли монете или деньгам сделки перейти между двумя биржами. Два правила:
//! (1) venue.cex_cex = нет у биржи X — X не торгует с другими биржами в ОБЕ стороны: ни как биржа, где сделка
//! начинается (X → любая биржа), ни как получатель (любая биржа → X, в том числе перегон монеты на X), в том числе через
//! кошелёк DEX; не задана — да (решение 214). (2) venue.cex_cex_deny у биржи A (список бирж) — ОДНОСТОРОННИЙ запрет:
//! монета или деньги сделки не уходят с A на биржу из списка A (продажа, перегон, в том числе через кошелёк DEX).
//! Дополнение (Тигран 10.11, п. 1): список проверяется у ВСЕХ бирж, через которые монета сделки уже прошла (биржа
//! начала, биржи переводов, где монета или деньги сейчас), а не только у биржи начала: переход на биржу B запрещён,
//! если B есть в списке любой из них (кроме самой B). Шаг A → B: B в списке A или у A или B venue.cex_cex = нет —
//! запрещён. Список биржи B на уход монеты на B не влияет. Не задан или пуст — исключений нет. Монета ещё не была ни
//! на одной бирже (сделка начата на DEX) — DEX → биржа, не затрагивается (п. 2). Торговля биржи с DEX не затрагивается.
//! Кто вызывает: rebuild_blacklist.rs (направления справочника, deny_pairs), risk.rs (can_open), executor.rs (перед переводом),
//! relay.rs и manual_relay_targets.rs (цели перегона), usdt_return_deal.rs (возврат USDT: return_block; повторы и
//! «Перегнать» — usdt_return_retry.rs, usdt_return_attempt.rs), usdt_return.rs (режим «отдельно», pair).
//! От чего зависит: settings (read, store), trade_ops.rs (вид площадки), deal.rs.
//! Настройки: venue.cex_cex, venue.cex_cex_deny (по площадке, обе необязательные).

use crate::core::types::{Direction, VenueId};
use crate::gateway::contract::VenueKind;
use crate::gateway::errors::ErrorCode;
use crate::settings::read::At;
use crate::settings::store::Store;

use super::deal::Deal;
use super::engine::Core;
use super::registry::Presence;

/// Имя настройки списка «не взаимодействовать» (решение 249).
pub const DENY: &str = "venue.cex_cex_deny";

/// Область «площадка». Принимает: id. Ошибок нет.
fn at(v: &str) -> At<'_> {
    At {
        venue: Some(v),
        coin: None,
    }
}

/// venue.cex_cex биржи (решение 214: не задана — да). Принимает: настройки, площадку. Ошибок нет.
pub fn on(s: &Store, v: &str) -> bool {
    s.flag("venue.cex_cex", at(v)).unwrap_or(true)
}

/// Список `from` запрещает уход на `to` (решение 249, одностороннее правило). Не задан — нет; хранится в неверном
/// виде — запрещено (не торгуем вслепую). Принимает: настройки, биржу начала, биржу-цель. Возвращает: причину или None.
pub fn listed(s: &Store, from: &str, to: &str) -> Option<String> {
    match s.list(DENY, at(from)) {
        Ok(l) if l.iter().any(|x| x == to) => Some(format!(
            "{from} не взаимодействует с {to} (список {DENY} у {from})"
        )),
        Ok(_) => None,
        Err(e) if e.code == ErrorCode::NotConfigured => None,
        Err(e) => Some(format!("{DENY} у {from}: {}", e.message)),
    }
}

/// Решение 249: направления биржа → биржа монеты, запрещённые списком биржи покупки (поле CoinEntry.cex_cex_deny; биржа
/// монеты — Presence.info есть). Принимает: настройки, площадки монеты. Возвращает: пары (покупка, продажа). Ошибок нет.
pub fn deny_pairs(s: &Store, presence: &[Presence]) -> Vec<(VenueId, VenueId)> {
    let cex: Vec<&VenueId> = presence
        .iter()
        .filter(|p| p.info.is_some())
        .map(|p| &p.venue)
        .collect();
    let mut out = vec![];
    for a in &cex {
        for b in cex.iter().filter(|b| *b != a) {
            if listed(s, &a.0, &b.0).is_some() {
                out.push(((*a).clone(), (*b).clone()));
            }
        }
    }
    out
}

/// Обе ли стороны разрешают биржа ↔ биржа (правило 1, в обе стороны). Принимает: настройки, две биржи.
/// Возвращает: причину запрета или None.
pub fn flags(s: &Store, a: &str, b: &str) -> Option<String> {
    [a, b]
        .into_iter()
        .find(|v| !on(s, v))
        .map(|v| format!("биржа ↔ биржа выключена у {v} (venue.cex_cex = нет)"))
}

/// Можно ли сделке, начатой на бирже `from`, увести монету или деньги на биржу `to` (обе — биржи): правило 1 и список
/// `from`. Принимает: настройки, биржу начала, биржу-цель. Возвращает: причину запрета или None.
pub fn pair(s: &Store, from: &str, to: &str) -> Option<String> {
    flags(s, from, to).or_else(|| listed(s, from, to))
}

/// Площадка — биржа (по реестру ядра). Ошибок нет: нет в реестре — не биржа.
fn is_cex(core: &Core, v: &VenueId) -> bool {
    super::trade_ops::kind(core, v).ok() == Some(VenueKind::Cex)
}

/// Направление биржа → биржа разрешено (risk::can_open, executor.rs). Одна из сторон DEX — да.
/// Принимает: ядро, направление. Ошибки: причина запрета.
pub fn direction_ok(core: &Core, d: &Direction) -> Result<(), String> {
    if !is_cex(core, &d.buy_on) || !is_cex(core, &d.sell_on) {
        return Ok(());
    }
    match pair(&core.inner.deps.settings, &d.buy_on.0, &d.sell_on.0) {
        Some(why) => Err(why),
        None => Ok(()),
    }
}

/// Биржи, через которые монета сделки уже прошла: площадка покупки, площадки переводов (deal.transfers), где монета
/// сейчас. Принимает: ядро, сделку. Возвращает: биржи без повторов. Ошибок нет.
pub fn path_cexes(core: &Core, deal: &Deal) -> Vec<VenueId> {
    let mut out: Vec<VenueId> = vec![];
    let all = std::iter::once(&deal.direction.buy_on)
        .chain(deal.transfers.iter().map(|t| &t.venue))
        .chain(std::iter::once(&deal.at));
    for v in all {
        if !out.contains(v) && is_cex(core, v) {
            out.push(v.clone());
        }
    }
    out
}

/// Можно ли перегнать монету сделки на `to` (перегон застрявшей монеты, «Продать и перегнать»). `to` — DEX: да.
/// `to` — биржа: правило 1 с каждой биржей, через которую монета уже прошла (кроме самой `to`), и список каждой из них
/// (дополнение 10.11, п. 1: не только биржи покупки). Монета ещё не была ни на одной бирже (сделка начата на DEX и
/// монета на DEX) — это DEX → биржа, не затрагивается (п. 2). Принимает: ядро, сделку, цель. Возвращает: причину
/// запрета или None.
pub fn relay_block(core: &Core, deal: &Deal, to: &VenueId) -> Option<String> {
    if !is_cex(core, to) {
        return None;
    }
    let s = &core.inner.deps.settings;
    let path = path_cexes(core, deal);
    let flag = path
        .iter()
        .filter(|c| *c != to)
        .find_map(|c| flags(s, &c.0, &to.0));
    flag.or_else(|| path_listed(s, &path, to))
}

/// Дополнение 10.11 (п. 1): `to` в списке venue.cex_cex_deny любой биржи пути (кроме самой `to`). Принимает: настройки,
/// биржи пути, цель. Возвращает: первую причину запрета или None.
fn path_listed(s: &Store, path: &[VenueId], to: &VenueId) -> Option<String> {
    path.iter()
        .filter(|c| *c != to)
        .find_map(|c| listed(s, &c.0, &to.0))
}

/// Возврат выручки USDT сделки с `from` на `to` — usdt_return_deal.rs, повторы и «Перегнать»
/// (дополнение 10.11, п. 4: перепроверка на каждой попытке). `to` не биржа или `from` = `to` — да. Правило 1 — у `from`
/// и `to`, если обе биржи (как в решении 249). Списки venue.cex_cex_deny: возврат на биржу покупки этой же сделки
/// (`to` = buy_on) ими НЕ ограничивается — это часть уже разрешённой сделки, запрет не взаимный (Тигран 10.11, п. 6);
/// на любую другую биржу — списки каждой биржи пути монеты сделки и `from` (кроме самой `to`, п. 1).
/// Принимает: ядро, сделку, откуда, куда. Возвращает: причину запрета или None.
pub fn return_block(core: &Core, deal: &Deal, from: &VenueId, to: &VenueId) -> Option<String> {
    if from == to || !is_cex(core, to) {
        return None;
    }
    let s = &core.inner.deps.settings;
    if is_cex(core, from)
        && let Some(why) = flags(s, &from.0, &to.0)
    {
        return Some(why);
    }
    // Тигран 10.11, п. 6: возврат выручки на биржу начала сделки — списками не ограничивается.
    if *to == deal.direction.buy_on {
        return None;
    }
    let mut path = path_cexes(core, deal);
    if is_cex(core, from) && !path.contains(from) {
        path.push(from.clone());
    }
    path_listed(s, &path, to)
}
