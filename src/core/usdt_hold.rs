//! Файл: core/usdt_hold.rs
//! Что делает: решение 405 (t191, Тигран 18.11: «рассчитывать на ожидаемые средства не нужно; да, резервировать»;
//! «не вернулись» — тоже резервировать) — резерв USDT возвратов выручки: покупка, ручной перевод и чужой перегон берут
//! только реально свободное МИНУС все ждущие и идущие возвраты на этой площадке (у DEX — этом кошельке).
//!
//! В резерве (у каждой сделки — по площадке, где лежат USDT):
//! - отложенная часть общего возврата группы докупки — Deal.group.wait (ставит coin_group.rs hold_parts, снимает
//!   coin_group_flush.rs при старте общего возврата и ledger_load.rs при запуске);
//! - части возврата (общего и одиночного, решение 227: Deal.usdt_return, Deal.usdt_return_parts): «идёт» (Pending — и
//!   остаток не ушедших кусков, usdt_return_pieces.rs), «ждёт повтора» (Retrying), «не вернулись» (NotReturned — ждут
//!   человека; снимается его кнопкой: «Перегнать» → «идёт», «Перегнано вручную» → Manual, manual_usdt.rs).
//!
//! Не в резерве: «вернулись» (Done), «перегнано вручную» (Manual) — денег на площадке нет.
//! Ушедший вывод: у «идёт» из самой части не видно, ушёл ли вывод. Признаки по коду: (1) ревью 4, Б2 — площадка
//! приняла вывод (ответ на запрос вывода, transfers.rs → usdt_accepted.rs; в учёте сделки — ревью 5, Н1: и у «не
//! вернулись», ставшей такой после перезапуска) — сумма вывода вычитается сразу; (2) ревью
//! 3, Н2 — ожидание зачисления этого перегона (credit_watch.rs) с хешем вывода от отправителя — его ожидаемый приход
//! (если принятие не записано). До ответа площадки на запрос вывода — резерв целиком (деньги ещё на площадке).
//! Свой резерв (ревью 3, Н3): перегон части видит свободное минус резерв ДРУГИХ сделок (held_except: своя сделка — по
//! ID перегона, relay_owner.rs), свой тратит.
//! Кошелёк: у DEX — сделки того же кошелька (Deal.dex_account, решение 132; неизвестен — в резерве); у биржи — все
//! сделки площадки. Вид площадки неизвестен — как у DEX. От exec.top_up не зависит (базовое поведение).
//! Стоимость (ревью 3, М2): только сделки набора unsettled.rs (идущие и с незаконченным возвратом), один замок `deals`
//! на проход по ним и короткий замок ожиданий зачисления, если есть «идёт».
//! Кто вызывает: limits.rs (buy_free), usdt_transfer_plan.rs, usdt_return_attempt.rs, usdt_relay_retry.rs.
//! От чего зависит: unsettled.rs, engine.rs (deals), usdt_accepted.rs, credit_watch.rs, relay_owner.rs, trade_ops.rs (вид площадки),
//! deal.rs, usdt_return_parts.rs, usdt_return_state.rs.
//! Настройки: нет.

use rust_decimal::Decimal;

use crate::core::types::{DexAccountId, VenueId};
use crate::gateway::contract::VenueKind;

use super::deal::Deal;
use super::engine::{Core, lk};
use super::usdt_return_state::UsdtReturn;

/// Резерв площадки: сумма и за кем.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Held {
    /// USDT в резерве возвратов.
    pub amount: Decimal,
    /// Сделка (ID) и что ждёт (для текста причины).
    pub deals: Vec<(String, String)>,
    /// То же с суммой каждой строки (экран резервов, usdt_hold_view.rs; ревью 4, Н2).
    pub lines: Vec<(String, String, Decimal)>,
    /// Для текста отказа (требование Тиграна 18.11; ставит limits.rs buy_free): прочитанное свободно до резерва и
    /// площадка словами (у DEX — с кошельком). None — не читалось.
    pub free: Option<Decimal>,
    pub place: Option<String>,
}

/// Строка резерва одной сделки: сумма (уже за вычетом принятых площадкой выводов — usdt_accepted.rs), что ждёт,
/// «идёт» ли (ещё и хеш-признак), перегоны с принятым выводом (у них хеш не считается второй раз).
struct Line {
    deal: String,
    amount: Decimal,
    what: String,
    pending: bool,
    accepted: Vec<String>,
}

/// Резерв одной сделки на площадке `venue`. Ошибок нет.
fn of_deal(d: &Deal, venue: &VenueId) -> Vec<Line> {
    let line = |amount: Decimal, what: &str, pending: bool| Line {
        deal: d.id.clone(),
        amount,
        what: what.to_string(),
        pending,
        accepted: vec![],
    };
    // Ревью 4 Б2, ревью 5 Н1: принятый площадкой вывод части (в учёте сделки) — деньги уже списаны: и у «идёт», и у
    // ставшей после перезапуска «не вернулись».
    let minus_accepted = |mut l: Line, from: &VenueId| {
        let (sent, ids) = super::usdt_accepted::sent_of(d, from, l.amount);
        l.amount = (l.amount - sent).max(Decimal::ZERO);
        l.accepted = ids;
        l
    };
    // Требование Тиграна 18.11: что ждёт — словами (монета, откуда → куда, куда возврат и почему).
    let r = super::deal_words::route(&d.direction);
    let to = |v: &VenueId| crate::notify::human::venue(&v.0);
    let mut out = vec![];
    if d.is_open()
        && let Some(v) = d.plan.get(super::settlement_cycle::FIELD)
        && v.pointer("/market/cex").and_then(|s| s.as_str()) == Some(&venue.0)
        && let Some(cap) = v
            .get("quote_cap")
            .and_then(|s| serde_json::from_value::<Decimal>(s.clone()).ok())
    {
        out.push(line(
            (cap - d.invested).max(Decimal::ZERO),
            "на конверсию и покупку",
            false,
        ));
    }
    if let Some(g) = d.group.as_ref()
        && let Some((from, amount)) = g.wait.as_ref()
        && from == venue
    {
        let w = format!(
            "{r}: выручка ждёт общего возврата группы {} на {}",
            g.id,
            to(&d.direction.buy_on)
        );
        out.push(line(*amount, &w, false));
    }
    for p in super::usdt_return_parts::all(d) {
        match p {
            UsdtReturn::Pending {
                amount,
                from,
                to: t,
                ..
            } if from == venue => {
                let w = format!("{r}: перегон выручки на {} идёт", to(t));
                out.push(minus_accepted(line(*amount, &w, true), from));
            }
            UsdtReturn::Retrying {
                amount,
                from,
                to: t,
                why,
                ..
            } if from == venue => {
                let w = format!("{r}: перегон выручки на {} ждёт повтора ({why})", to(t));
                out.push(line(*amount, &w, false));
            }
            UsdtReturn::NotReturned {
                amount, at, why, ..
            } if at == venue => {
                let w = format!(
                    "{r}: выручка не вернулась на {} ({why}) — ждёт кнопки «Перегнать» или «Перегнано вручную»",
                    to(&d.direction.buy_on)
                );
                out.push(minus_accepted(line(*amount, &w, false), at));
            }
            _ => {}
        }
    }
    out
}

/// Что делает: резерв возвратов USDT на площадке `venue` (у DEX — кошелёк `acc`). Принимает: ядро, площадку, счёт DEX
/// (None — биржа или счёт неизвестен). Возвращает: резерв. Ошибок нет.
pub fn held(core: &Core, venue: &VenueId, acc: Option<&DexAccountId>) -> Held {
    held_except(core, venue, acc, None)
}

/// Что делает: резерв возвратов на площадке без резерва сделки `own` (свой резерв перегон тратит, ревью 3, Н3).
/// Принимает: ядро, площадку, счёт DEX, ID своей сделки (None — все). Возвращает: резерв. Ошибок нет. Замки: deals (по
/// набору unsettled), затем отдельно — ожидания зачисления; не вложенно.
pub fn held_except(
    core: &Core,
    venue: &VenueId,
    acc: Option<&DexAccountId>,
    own: Option<&str>,
) -> Held {
    let ids = super::unsettled::ids(core);
    if ids.is_empty() {
        return Held::default();
    }
    let cex = super::trade_ops::kind(core, venue) == Ok(VenueKind::Cex);
    let mut lines = vec![];
    let mut gone = vec![];
    {
        let deals = lk(&core.inner.deals);
        for id in &ids {
            let Some(d) = deals.get(id) else {
                gone.push(id.clone());
                continue;
            };
            if !super::unsettled::keep(d) {
                gone.push(id.clone());
                continue;
            }
            if own == Some(d.id.as_str()) {
                continue;
            }
            // У DEX — тот же кошелёк (неизвестный — в резерве); у биржи — любой.
            let same =
                cex || acc.is_none() || d.dex_account.is_none() || d.dex_account.as_ref() == acc;
            if same {
                lines.extend(of_deal(d, venue));
            }
        }
    }
    super::unsettled::drop_ids(core, &gone);
    if lines.iter().any(|l| l.pending) {
        let st = super::credit_watch::lock(core);
        for l in lines.iter_mut().filter(|l| l.pending) {
            // Хеш вывода от отправителя (ревью 3, Н2) — у перегонов, чей принятый вывод не записан (of_deal).
            let hashed: Decimal = st
                .list
                .values()
                .filter(|e| e.from == *venue && e.tx.is_some() && !l.accepted.contains(&e.deal))
                .filter(|e| super::relay_owner::is_relay(&l.deal, &e.deal))
                .map(|e| e.amount)
                .sum();
            l.amount = (l.amount - hashed).max(Decimal::ZERO);
        }
    }
    let mut out = Held::default();
    for l in lines.into_iter().filter(|l| l.amount > Decimal::ZERO) {
        out.amount += l.amount;
        out.lines.push((l.deal.clone(), l.what.clone(), l.amount));
        out.deals.push((l.deal, l.what));
    }
    out.deals.sort();
    out.lines.sort();
    out
}

/// Что делает: своя сделка перегона USDT по его ID (relay_owner.rs; ищется среди набора unsettled — у перегона часть
/// «идёт»). Принимает: ядро, ID перегона. Возвращает: ID сделки; None — перегон вне сделки (ручной перевод «tu…»).
/// Ошибок нет.
pub fn owner_of(core: &Core, relay: &str) -> Option<String> {
    let ids = super::unsettled::ids(core);
    super::relay_owner::owner(ids.iter(), relay)
}

/// Что делает: свободно для покупки и перевода = прочитанное свободно − резерв (не меньше 0). Принимает: свободно,
/// резерв. Ошибок нет.
pub fn spendable(free: Decimal, h: &Held) -> Decimal {
    (free - h.amount).max(Decimal::ZERO)
}

pub use super::usdt_hold_text::suffix;
