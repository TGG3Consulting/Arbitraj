//! Файл: core/withdraw_after.rs
//! Что делает: решение 253, дополнение t89 (Тигран 11.11) — вывод сразу после своей сделки на площадке. Живой случай
//! 11.11, сделка 20261111122340181OPEN (DEX → Binance): продажа на Binance исполнилась 12:24:21.980, возврат USDT начат
//! 12:24:21.981, Binance отказал 12:24:22.582 «−4026 [031033] User has insufficient balance»; защита решения 253 (пауза,
//! ожидание события, повтор −4026/−4024) действовала только на вывод монеты после покупки сделки.
//! Здесь:
//! - from_trade: когда пауза venue.withdraw_min_delay_ms кончается — от исполнения покупки сделки на площадке (как было,
//!   у всех бирж) и от последней своей сделки (покупки ИЛИ продажи) площадки по этому активу — если адаптер её сообщает
//!   (контракт since_own_trade: Binance; у прочих — None, без изменений). Для USDT — продажа, давшая выручку;
//! - deal_of: сделка по ID вывода — у возврата USDT ID вывода = ID сделки + метка попытки («u», «u<N>» —
//!   usdt_return_job.rs / usdt_return_retry.rs, «m<цифры>» — «Перегнать», manual_usdt.rs); строки хронологии ожидания и
//!   повторов (withdraw_wait.rs, transfer_locked.rs) пишутся в закрытую сделку, а не только в журнал.
//!
//! Кто вызывает: core/withdraw_wait.rs (pause, note), core/transfer_locked.rs (note).
//! От чего зависит: engine.rs (сделки), gateway/contract.rs (since_own_trade).
//! Настройки: нет (паузу задаёт venue.withdraw_min_delay_ms — читает withdraw_wait.rs).

use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::core::types::{Asset, VenueId};
use crate::gateway::contract::Venue;

use super::engine::{Core, lk};

/// С какой сделки отсчитана пауза: покупка сделки или своя сделка площадки по активу.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Since {
    /// Покупка сделки на этой площадке (TradeResult.finished_at) — как до t89.
    DealBuy,
    /// Последняя своя сделка площадки по активу (контракт since_own_trade).
    OwnTrade,
}

/// Что делает: момент своей сделки, от которого считается пауза: позже из «покупка сделки на площадке» и «своя сделка
/// по активу» (адаптер). Принимает: ядро, ID вывода (сделка или сделка + метка), площадку, её адаптер, актив (тикер
/// площадки), сейчас. Возвращает: (момент, откуда); None — ни того ни другого. Ошибок нет.
pub fn from_trade(
    core: &Core,
    id: &str,
    from: &VenueId,
    gw: &Arc<dyn Venue>,
    asset: &Asset,
    now: DateTime<Utc>,
) -> Option<(DateTime<Utc>, Since)> {
    let buy = {
        let deals = lk(&core.inner.deals);
        deal_key(|k| deals.contains_key(k), id)
            .and_then(|k| deals.get(&k))
            .and_then(|d| d.buy.as_ref())
            .filter(|b| &b.request.venue == from)
            .map(|b| (b.finished_at, Since::DealBuy))
    };
    let own = gw
        .since_own_trade(asset)
        .and_then(|t| chrono::Duration::from_std(t).ok())
        .map(|t| (now - t, Since::OwnTrade));
    later(buy, own)
}

/// Позднее из двух моментов (чистая функция). Ошибок нет.
pub fn later(
    a: Option<(DateTime<Utc>, Since)>,
    b: Option<(DateTime<Utc>, Since)>,
) -> Option<(DateTime<Utc>, Since)> {
    match (a, b) {
        (Some(x), Some(y)) => Some(if y.0 > x.0 { y } else { x }),
        (x, y) => x.or(y),
    }
}

/// Что делает: ID сделки по ID вывода (чистая функция): сам ID, если сделка есть; иначе ID без метки попытки возврата
/// USDT в конце — «u», «u<цифры>» или «m<цифры>» (см. шапку). Принимает: «есть ли такая сделка», ID вывода.
/// Возвращает: ID сделки; None — не сделка (например, перегон «отдельно» — свой ID). Ошибок нет.
pub fn deal_key(has: impl Fn(&str) -> bool, id: &str) -> Option<String> {
    if has(id) {
        return Some(id.to_string());
    }
    let digits = id.len() - id.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    let head = &id[..id.len() - digits];
    let tag = head.chars().last()?;
    let ok = tag == 'u' || (tag == 'm' && digits > 0);
    let deal = &head[..head.len() - 1];
    (ok && !deal.is_empty() && has(deal)).then(|| deal.to_string())
}

/// ID сделки по ID вывода среди сделок ядра (deal_key). Ошибок нет.
pub fn deal_of(core: &Core, id: &str) -> Option<String> {
    let deals = lk(&core.inner.deals);
    deal_key(|k| deals.contains_key(k), id)
}
