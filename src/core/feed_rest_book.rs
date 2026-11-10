//! Файл: core/feed_rest_book.rs
//! Что делает: t79 (Тигран 10.11: согласовано; дополнение к решениям 202/213 «202/213 (дополнение 10.11)») — тихая
//! пара биржи ПО СОДЕРЖИМОМУ (MEXC, BingX, KuCoin), признанная «книга не менялась с переподписки» (feed_rest_ref.rs),
//! торгуется по стакану из ТОГО ЖЕ ответа REST, а не стоит без стакана до кадра. Ставит уровни ответа:
//! 1) стаканом пары в адаптере (контракт book_from_rest): время кадра пары — получение ответа (recv_ms), подтверждения
//!    тихой пары нет — котировка и стакан ордера (решение 238, quote_snapshot) берут его не дольше
//!    venue.book_stale_ms от получения ответа; живость соединения обязательна; кадр пары после отправки запроса —
//!    не ставится (стакан потока новее);
//! 2) в состояние рынка ядра (как событие стакана) — только если стакан ядра этой пары не новее отправки запроса
//!    (кадр, уже применённый ядром, главнее); затем пересчёт монеты (tick.rs schedule), как на кадр.
//!
//! Живость потока площадки (feed_live.rs) снимок REST НЕ отмечает: это не кадр потока.
//! Адаптер не умеет (NotApplicable) или не поставил — ошибка с причиной, пара не торгуется до кадра (как раньше);
//! строку журнала пишет feed_watch_act.rs.
//! Кто вызывает: core/feed_watch_act.rs (Action::Unchanged с book).
//! От чего зависит: feed_rest_ref.rs (RestBook), engine.rs (Core, wr), market_state.rs, venue_ticker.rs, tick.rs
//! (schedule), gateway (contract book_from_rest, events).
//! Настройки: нет своих; свежесть — venue.book_stale_ms и feed.stale_after_ms площадки (проверяет адаптер).

use crate::core::types::VenueId;
use crate::gateway::contract::Venue;
use crate::gateway::events::MarketEvent;

use super::engine::{Core, wr};
use super::feed_rest_ref::RestBook;
use super::market_state::Touched;

/// Ставит ответ REST стаканом пары (порядок — в шапке). Принимает: ядро, площадку, адаптер, тикер площадки, уровни
/// ответа со временем отправки и получения. Возвращает: Ok — поставлен в адаптере. Ошибки (текст причины): адаптер не
/// умеет или не поставил (book_from_rest).
pub async fn install(
    core: &Core,
    venue: &VenueId,
    v: &dyn Venue,
    ticker: &str,
    b: &RestBook,
) -> Result<(), String> {
    let book = v
        .book_from_rest(ticker, &b.levels, b.sent_ms, b.recv_ms)
        .map_err(|e| e.message)?;
    let touched = {
        let mut m = wr(&core.inner.market);
        let newer = m
            .book(venue, ticker)
            .is_some_and(|x| x.received_at.timestamp_millis() > b.sent_ms);
        (!newer).then(|| m.apply(MarketEvent::Book(book)))
    };
    if let Some(Touched::Book { first, .. }) = touched {
        if first {
            core.request_rebuild();
        }
        if let Some(a) = super::venue_ticker::asset_by_venue_ticker(core, venue, ticker) {
            super::tick::schedule(core, a, venue.clone());
        }
    }
    Ok(())
}
