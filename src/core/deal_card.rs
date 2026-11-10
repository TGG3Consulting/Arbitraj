//! Файл: core/deal_card.rs
//! Что делает: решение 251 (Тигран 10.11: «везде во всех алертах писать путь, например BingX → DEX → BingX … всегда
//! конкретно указывать, где именно сейчас застряла монета») — карточка сделки для шаблона алерта
//! (notify/alert_card.rs::DealCard) только из учёта сделки:
//! - монета: тикер и короткий адрес контракта BSC (если площадка его сообщила);
//! - путь: площадка покупки → площадки, где монета была по факту переводов (Deal.transfers: вывод и зачисление, в т.ч.
//!   перегоны) → где монета сейчас (Deal.at); пока сделка открыта и монета не дошла до площадки продажи плана — она
//!   «(план)»; возврат выручки начат — «→ <куда> (USDT)»; DEX — с кошельком сделки (Deal.dex_account);
//! - где сейчас: по шагу сделки — монета на площадке / в пути / у человека / монет нет и где USDT.
//!
//! Логику не меняет — только то, что видит человек.
//! Кто вызывает: core/deal_text.rs, stuck.rs, manual.rs, manual_feed.rs (алерты), core/engine.rs (поиск карточки для
//! notify — install).
//! От чего зависит: deal.rs, usdt_return_state.rs, notify/human.rs (имена площадок, количество, адрес),
//! notify/alert_card.rs (DealCard).
//! Настройки: нет.

use crate::notify::alert_card::DealCard;
use crate::notify::human as h;

use super::deal::{Deal, Step};
use super::usdt_return_state::UsdtReturn;

/// Площадка DEX (id адаптера, venues/bsc_dex) — у неё показывается кошелёк сделки.
const DEX: &str = "bsc_dex";

/// Карточка сделки. Принимает: сделку. Возвращает: DealCard. Ошибок нет.
pub fn card(d: &Deal) -> DealCard {
    DealCard {
        coin: coin(d),
        path: path(d),
        now: now(d),
    }
}

/// «PIG (0x1234…abcd)» или тикер, если адреса нет. Ошибок нет.
pub fn coin(d: &Deal) -> String {
    let a = &d.direction.asset;
    match &a.bsc_contract {
        Some(c) => format!("{} ({})", a.ticker, h::addr(&c.to_string())),
        None => a.ticker.clone(),
    }
}

/// Имя площадки для пути: DEX — с кошельком сделки. Принимает: сделку, id площадки. Ошибок нет.
fn place(d: &Deal, id: &str) -> String {
    match (&d.dex_account, id == DEX) {
        (Some(w), true) => format!("{} (кошелёк {w})", h::venue(id)),
        _ => h::venue(id),
    }
}

/// Путь сделки по факту учёта. Принимает: сделку. Возвращает: «BingX → DEX (кошелёк w2) → BingX (USDT)». Ошибок нет.
pub fn path(d: &Deal) -> String {
    let mut ids: Vec<&str> = vec![d.direction.buy_on.0.as_str()];
    let seq = d
        .transfers
        .iter()
        .map(|t| t.venue.0.as_str())
        .chain(std::iter::once(d.at.0.as_str()));
    for id in seq {
        if ids.last() != Some(&id) {
            ids.push(id);
        }
    }
    let mut out: Vec<String> = ids.iter().map(|id| place(d, id)).collect();
    let sell = &d.direction.sell_on.0;
    if d.is_open() && !ids.contains(&sell.as_str()) {
        out.push(format!("{} (план)", place(d, sell)));
    }
    if let Some(to) = d.usdt_return.as_ref().and_then(usdt_to) {
        out.push(format!("{} (USDT)", place(d, to)));
    }
    out.join(" → ")
}

/// Куда идёт возврат выручки USDT. Ошибок нет.
fn usdt_to(r: &UsdtReturn) -> Option<&str> {
    match r {
        UsdtReturn::Pending { to, .. }
        | UsdtReturn::Done { to, .. }
        | UsdtReturn::Retrying { to, .. }
        | UsdtReturn::Manual { to, .. } => Some(&to.0),
        UsdtReturn::NotReturned { .. } => None,
    }
}

/// Где сейчас монета или деньги сделки. Принимает: сделку. Ошибок нет.
pub fn now(d: &Deal) -> String {
    let coins = format!(
        "{} {} на {}",
        h::qty(d.qty),
        d.direction.asset.ticker,
        place(d, &d.at.0)
    );
    match d.step {
        Step::Buying => format!("покупка на {}", place(d, &d.direction.buy_on.0)),
        Step::Transfer => format!("в пути: перевод {coins}"),
        Step::Selling | Step::Stuck | Step::WaitingHuman => format!("монета: {coins}"),
        Step::Halted if d.qty > rust_decimal::Decimal::ZERO => {
            format!("монета: {coins} (сделка остановлена)")
        }
        Step::TakenOver => format!("монеты у вас: {coins}"),
        Step::Halted | Step::Closed => format!("монет нет; {}", usdt_now(d)),
    }
}

/// Где выручка USDT закрытой сделки (по состоянию возврата). Ошибок нет.
fn usdt_now(d: &Deal) -> String {
    match &d.usdt_return {
        None => format!("USDT — на {}", place(d, &d.at.0)),
        Some(UsdtReturn::Pending { from, to, .. }) => {
            format!("USDT в пути: {} → {}", place(d, &from.0), place(d, &to.0))
        }
        Some(UsdtReturn::Retrying { from, to, .. }) => format!(
            "USDT на {} — возврат на {} ждёт повтора",
            place(d, &from.0),
            place(d, &to.0)
        ),
        Some(UsdtReturn::Done { to, .. }) | Some(UsdtReturn::Manual { to, .. }) => {
            format!("USDT вернулись на {}", place(d, &to.0))
        }
        Some(UsdtReturn::NotReturned { at, .. }) => {
            format!("USDT не вернулись — лежат на {}", place(d, &at.0))
        }
    }
}
