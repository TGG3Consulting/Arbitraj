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
//! приняла вывод (ответ на запрос вывода, transfers.rs → usdt_accepted.rs) — сумма вывода вычитается сразу; (2) ревью
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
}

/// Строка резерва одной сделки: сумма, что ждёт, «идёт» ли (вычесть ушедший вывод).
struct Line {
    deal: String,
    amount: Decimal,
    what: String,
    pending: bool,
}

/// Резерв одной сделки на площадке `venue`. Ошибок нет.
fn of_deal(d: &Deal, venue: &VenueId) -> Vec<Line> {
    let line = |amount: Decimal, what: &str, pending: bool| Line {
        deal: d.id.clone(),
        amount,
        what: what.to_string(),
        pending,
    };
    let mut out = vec![];
    if let Some(g) = d.group.as_ref()
        && let Some((from, amount)) = g.wait.as_ref()
        && from == venue
    {
        out.push(line(
            *amount,
            &format!("общий возврат группы {}", g.id),
            false,
        ));
    }
    for p in super::usdt_return_parts::all(d) {
        match p {
            UsdtReturn::Pending { amount, from, .. } if from == venue => {
                out.push(line(*amount, "возврат идёт", true));
            }
            UsdtReturn::Retrying { amount, from, .. } if from == venue => {
                out.push(line(*amount, "возврат ждёт повтора", false));
            }
            UsdtReturn::NotReturned { amount, at, .. } if at == venue => {
                out.push(line(*amount, "не вернулись — ждут человека", false));
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
        // Ревью 4, Б2: вывод принят площадкой (ответ на запрос вывода — usdt_accepted.rs) — деньги уже списаны.
        let accepted: Vec<(Decimal, Vec<String>)> = lines
            .iter()
            .map(|l| {
                if l.pending {
                    super::usdt_accepted::sent(core, &l.deal, venue, l.amount)
                } else {
                    (Decimal::ZERO, vec![])
                }
            })
            .collect();
        let st = super::credit_watch::lock(core);
        for (l, (acc, ids)) in lines.iter_mut().zip(accepted) {
            if !l.pending {
                continue;
            }
            // Хеш вывода от отправителя (ревью 3, Н2) — у перегонов, чей принятый вывод не записан выше.
            let hashed: Decimal = st
                .list
                .values()
                .filter(|e| e.from == *venue && e.tx.is_some() && !ids.contains(&e.deal))
                .filter(|e| super::relay_owner::is_relay(&l.deal, &e.deal))
                .map(|e| e.amount)
                .sum();
            l.amount = (l.amount - acc - hashed).max(Decimal::ZERO);
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

/// Что делает: хвост текста причины (отказ «свободно 0», заметка ручного перевода, отказ перегона): сколько в резерве и
/// за кем — та же сумма, что вычтена (М7: текст из того же Held). Принимает: резерв, тикер котируемой монеты.
/// Возвращает: пустую строку — резерва нет. Ошибок нет.
pub fn suffix(h: &Held, quote: &str) -> String {
    if h.amount <= Decimal::ZERO {
        return String::new();
    }
    let who: Vec<String> = h.deals.iter().map(|(d, w)| format!("{d} ({w})")).collect();
    format!(
        " — ещё {} {quote} в резерве возвратов USDT: {}; их не трогаем (решение 405)",
        h.amount,
        who.join(", ")
    )
}
