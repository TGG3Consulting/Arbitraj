//! Файл: core/exec_book.rs
//! Что делает: решение 238 (Тигран 09.11, для всех бирж; живой случай XTER 08.11, сделки 20261108213430926XTER и
//! 20261108214123430XTER: котировка продажи на MEXC прошла по стакану, которому 32 / 55 с, а худшая цена ордера
//! отказала «стакан устарел» — и 3 повтора с паузой 0 по тому же стакану, затем алерт) — стакан биржи для ОРДЕРА: один
//! источник и одна мера свежести на обоих шагах — котировка исполнения (trade_ops::sell_value) и худшая цена ордера
//! (worst.rs worst_order).
//! Было (факт кода): котировка — адаптер (quote: кадр пары, в том числе кадр с тем же снимком — touch, решение 109,
//! или подтверждение тихой пары — решение 213), худшая цена — стакан ядра по времени ПОСЛЕДНЕГО ИЗМЕНЁННОГО снимка
//! (received_at): адаптер одинаковый снимок ядру не отдаёт, поэтому у пары, которая шлёт кадры без изменений, ядро
//! видит «устарел», а адаптер — «свежий».
//! Стало — порядок:
//! 1. снимок REST этой пары и стороны, взятый на пути ордера, не старше venue.book_stale_ms площадки, — он (котировка
//!    и худшая цена видят один и тот же снимок; второй запрос не идёт);
//! 2. снимок стакана адаптера (контракт quote_snapshot: та же мера, что у котировки расчёта — кадр пары не старше
//!    venue.book_stale_ms или действующее подтверждение тихой пары);
//! 3. адаптер ответил StaleData — снимок по REST (контракт quote_rest), запоминается здесь;
//! 4. REST не ответил — один повтор через exec.rest_book_retry_ms; снова нет (или пауза не задана) — ошибка с
//!    причиной без кода безопасного повтора: продажа уходит в алерт человеку (sell_unsent.rs Retry::No), а не в
//!    повторы по тому же устаревшему стакану.
//!
//! Решение 238 (ревью, п. 1): ордер биржи отправлен (trade_ops.rs trade_seen, после x.trade) — снимок REST этой пары и
//! стороны забывается (forget): IOC по худшей цене снимка мог не исполниться или исполниться частично (съеденные нами
//! уровни ещё лежат в снимке) — повтор и «продажа здесь» берут свежий стакан, а не тот же снимок до venue.book_stale_ms.
//!
//! Кто вызывает: core/trade_ops.rs (sell_value — котировка биржи; trade_seen — forget после ордера), core/worst.rs
//! (worst_order — худшая цена биржи).
//! От чего зависит: gateway (registry, contract quote_snapshot / quote_rest, frozen, patience exec_scope),
//! venue_ticker.rs, op_error.rs, settings, journal.
//! Настройки: venue.book_stale_ms (по площадке, обязательна), exec.rest_book_retry_ms (по площадке, необязательна).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use crate::core::types::{Asset, Side, VenueId};
use crate::gateway::errors::{ErrorCode, GatewayError};
use crate::gateway::frozen::QuoteSnapshot;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::engine::Core;
use super::op_error::OpError;

/// Снимки REST по (площадка, тикер площадки, покупка?) — последний взятый на пути ордера.
pub type Cache = Mutex<HashMap<(VenueId, String, bool), QuoteSnapshot>>;

/// Стакан для ордера: снимок и откуда он.
#[derive(Clone)]
pub struct Book {
    /// Снимок (стакан и план комиссии — gateway/frozen.rs).
    pub snap: QuoteSnapshot,
    /// true — снимок REST (решение 238), false — стакан потока адаптера.
    pub rest: bool,
}

/// Настройка паузы перед повтором REST.
pub const RETRY: &str = "exec.rest_book_retry_ms";

/// Стакан биржи для ордера (порядок — в шапке). Принимает: ядро, площадку, монету ЯДРА, сторону.
/// Возвращает: Book. Ошибки: площадки нет; не задана venue.book_stale_ms (NotConfigured); ошибки адаптера, кроме
/// StaleData, — с их кодом; REST не ответил после повтора — без кода (не безопасный повтор, алерт).
pub async fn book(core: &Core, v: &VenueId, a: &Asset, side: Side) -> Result<Book, OpError> {
    let x = core
        .inner
        .deps
        .gw
        .get(v)
        .await
        .map_err(|e| OpError::from(&e))?;
    let on = super::venue_ticker::on_venue(core, a, v);
    let max = stale_ms(core, v)?;
    let key = (v.clone(), on.ticker.clone(), side == Side::Buy);
    if let Some(s) = cached(core, &key, max) {
        return Ok(Book {
            snap: s,
            rest: true,
        });
    }
    let stream = match crate::gateway::patience::exec_scope(x.quote_snapshot(&on, side)).await {
        Ok(s) => {
            return Ok(Book {
                snap: s,
                rest: false,
            });
        }
        Err(e) if e.code == ErrorCode::StaleData => e,
        Err(e) => return Err(OpError::from(&e)),
    };
    let first = match crate::gateway::patience::exec_scope(x.quote_rest(&on, side)).await {
        Ok(s) => return Ok(keep(core, key, s, &stream).await),
        Err(e) => e,
    };
    let pause = setting(core, v, RETRY);
    let Some(ms) = pause else {
        return Err(failed(
            core,
            v,
            &stream,
            &format!("{}; повтор не настроен: не задана {RETRY}", first.message),
        )
        .await);
    };
    let text = format!(
        "{}: {}; снимок стакана по REST не получен: {} — повтор через {ms} мс",
        v.0, stream.message, first.message
    );
    core.inner
        .deps
        .log(Level::Warn, "core.exec_book", Some(&v.0), text, None)
        .await;
    tokio::time::sleep(Duration::from_millis(ms)).await;
    match crate::gateway::patience::exec_scope(x.quote_rest(&on, side)).await {
        Ok(s) => Ok(keep(core, key, s, &stream).await),
        Err(e) => {
            let why = format!("{}; повтор через {ms} мс: {}", first.message, e.message);
            Err(failed(core, v, &stream, &why).await)
        }
    }
}

/// Решение 238 (ревью, п. 1): забывает снимок REST пары и стороны после отправки ордера биржи — следующая попытка
/// (повтор правила шлюза, повтор продажи, «продажа здесь») берёт свежий стакан. Принимает: ядро, площадку, монету ЯДРА,
/// сторону. Ошибок нет (снимка не было — ничего не делает).
pub fn forget(core: &Core, v: &VenueId, a: &Asset, side: Side) {
    let on = super::venue_ticker::on_venue(core, a, v);
    let key = (v.clone(), on.ticker, side == Side::Buy);
    core.inner
        .exec_books
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remove(&key);
}

/// venue.book_stale_ms площадки — одна мера свежести для потока и снимка REST. Ошибка: не задана (ордера нет).
fn stale_ms(core: &Core, v: &VenueId) -> Result<i64, OpError> {
    let at = At {
        venue: Some(&v.0),
        coin: None,
    };
    match core.inner.deps.settings.integer("venue.book_stale_ms", at) {
        Ok(n) => Ok(n as i64),
        Err(e) => Err(OpError {
            code: Some(ErrorCode::NotConfigured),
            message: format!("{}: {} — стакан для ордера не проверить", v.0, e.message),
            sent: false,
            cancelled: false,
        }),
    }
}

/// Снимок REST из памяти, если он не старше max мс; старый — убирается. Ошибок нет.
fn cached(core: &Core, key: &(VenueId, String, bool), max: i64) -> Option<QuoteSnapshot> {
    let mut m = core
        .inner
        .exec_books
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let age = |s: &QuoteSnapshot| (chrono::Utc::now() - s.taken_at()).num_milliseconds();
    match m.get(key) {
        Some(s) if age(s) <= max => Some(s.clone()),
        Some(_) => {
            m.remove(key);
            None
        }
        None => None,
    }
}

/// Запоминает снимок REST и пишет в журнал, почему он взят. Возвращает: Book (rest). Ошибок нет.
async fn keep(
    core: &Core,
    key: (VenueId, String, bool),
    s: QuoteSnapshot,
    stream: &GatewayError,
) -> Book {
    let text = format!(
        "{}: {} — для ордера взят снимок стакана по REST ({})",
        key.0.0,
        stream.message,
        s.describe()
    );
    let venue = key.0.0.clone();
    core.inner
        .exec_books
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(key, s.clone());
    core.inner
        .deps
        .log(Level::Info, "core.exec_book", Some(&venue), text, None)
        .await;
    Book {
        snap: s,
        rest: true,
    }
}

/// Ошибка «стакан устарел, снимка REST нет» — без кода: продажа не повторяется по тому же стакану, а уходит в алерт с
/// причиной (sell_unsent.rs). Пишет в журнал Error. Принимает: ядро, площадку, ошибку потока, причину REST.
async fn failed(core: &Core, v: &VenueId, stream: &GatewayError, why: &str) -> OpError {
    let message = format!(
        "{}: {}; снимок стакана по REST не получен: {why}",
        v.0, stream.message
    );
    core.inner
        .deps
        .log(
            Level::Error,
            "core.exec_book",
            Some(&v.0),
            message.clone(),
            None,
        )
        .await;
    OpError::from(message)
}

/// Целая настройка площадки. None — не задана. Ошибок нет.
fn setting(core: &Core, v: &VenueId, name: &str) -> Option<u64> {
    let at = At {
        venue: Some(&v.0),
        coin: None,
    };
    core.inner.deps.settings.integer(name, at).ok()
}
