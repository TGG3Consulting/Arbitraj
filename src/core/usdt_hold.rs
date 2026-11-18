//! Файл: core/usdt_hold.rs
//! Что делает: решение 405 (t191, ответы Тиграна 18.11: «резервировать»; «рассчитывать на ожидаемые средства не
//! нужно») — резерв USDT возвратов выручки: покупка берёт только реально свободное МИНУС все ждущие и идущие возвраты
//! на этой площадке (у DEX — этом кошельке). В резерве:
//! - отложенные части общего возврата группы докупки — Deal.group.wait (площадка, сумма; ставит coin_group.rs
//!   hold_parts при закрытии, снимает coin_group_flush.rs при старте общего возврата и ledger_load.rs при запуске);
//! - части возврата (общего и одиночного, решение 227: Deal.usdt_return и Deal.usdt_return_parts), уже начатые:
//!   «идёт» (Pending — в том числе остаток не ушедших кусков общего возврата, usdt_return_pieces.rs) и «ждёт повтора»
//!   (Retrying). У «идёт» из состояния не видно, ушёл ли вывод: после запроса вывода и до «вернулись» сумма
//!   резервируется ещё раз поверх уже уменьшившегося свободного — покупка в это время берёт меньше (чужое не тратит).
//! Не в резерве: «вернулись», «перегнано вручную» (денег на площадке нет) и «не вернулись» (решает человек; вывод мог и
//! уйти — исход неизвестен) — вопрос в docs/TASKS.md (решение 405).
//! Кто вычитает: limits.rs (buy_free → buy_cap перед ордером и предпроверка precheck_balance.rs), usdt_transfer_plan.rs
//! (ручной перевод USDT: «можно вывести» − резерв). Сам возврат свой резерв тратит: перегон (usdt_relay_retry.rs,
//! usdt_return_attempt.rs) читает free_now без резерва и шлёт не больше суммы своей части (решение 227).
//! Кошелёк: у DEX — сделки того же кошелька (Deal.dex_account, решение 132; неизвестен — в резерве: не доказано, что
//! кошелёк другой); у биржи — все сделки площадки. Вид площадки неизвестен — как у DEX.
//! Резерв от включателя exec.top_up не зависит: после «да» → «нет» отложенные части идущих групп остаются в резерве до
//! общего возврата.
//! Кто вызывает: limits.rs, precheck_balance.rs (текст), usdt_transfer_plan.rs.
//! От чего зависит: engine.rs (deals), trade_ops.rs (вид площадки), deal.rs, deal_group.rs, usdt_return_parts.rs,
//! usdt_return_state.rs.
//! Настройки: нет (базовое поведение).

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
}

/// Резерв одной сделки на площадке `venue`: [(сумма, что ждёт)] (пусто — нет). Ошибок нет.
fn of_deal(d: &Deal, venue: &VenueId) -> Vec<(Decimal, String)> {
    let mut out = vec![];
    if let Some(g) = d.group.as_ref()
        && let Some((from, amount)) = g.wait.as_ref()
        && from == venue
    {
        out.push((*amount, format!("общий возврат группы {}", g.id)));
    }
    for p in super::usdt_return_parts::all(d) {
        match p {
            UsdtReturn::Pending { amount, from, .. } if from == venue => {
                out.push((*amount, "возврат идёт".into()));
            }
            UsdtReturn::Retrying { amount, from, .. } if from == venue => {
                out.push((*amount, "возврат ждёт повтора".into()));
            }
            _ => {}
        }
    }
    out
}

/// Что делает: резерв возвратов USDT на площадке `venue` (у DEX — кошелёк `acc`). Принимает: ядро, площадку, счёт DEX
/// (None — биржа или счёт неизвестен). Возвращает: резерв (пусто — нет). Ошибок нет. Замок: deals (проход по учёту в
/// памяти; await не держится).
pub fn held(core: &Core, venue: &VenueId, acc: Option<&DexAccountId>) -> Held {
    let cex = super::trade_ops::kind(core, venue) == Ok(VenueKind::Cex);
    let deals = lk(&core.inner.deals);
    let mut out = Held::default();
    for d in deals.values() {
        // У DEX — тот же кошелёк (неизвестный — в резерве); у биржи — любой.
        let same = cex || acc.is_none() || d.dex_account.is_none() || d.dex_account.as_ref() == acc;
        if !same {
            continue;
        }
        for (amount, what) in of_deal(d, venue) {
            out.amount += amount;
            out.deals.push((d.id.clone(), what));
        }
    }
    out.deals.sort();
    out
}

/// Что делает: свободно для покупки и перевода = прочитанное свободно − резерв (не меньше 0). Принимает: свободно,
/// резерв. Ошибок нет.
pub fn spendable(free: Decimal, h: &Held) -> Decimal {
    (free - h.amount).max(Decimal::ZERO)
}

/// Что делает: хвост текста причины (отказ «свободно 0», заметка ручного перевода): сколько в резерве и за кем — та же
/// сумма, что вычтена (М7: текст из того же Held). Принимает: резерв, тикер котируемой монеты. Возвращает: пустую
/// строку — резерва нет. Ошибок нет.
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
