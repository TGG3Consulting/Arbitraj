//! Файл: venues/gate/market.rs
//! Что делает: подписка на стаканы Gate по списку монет: делит монеты на соединения
//! (venue.ws_symbols_per_conn монет на соединение) и запускает по задаче на соединение.
//! Состояние пар монет и монеты, которыми можно торговать через API, сообщает ядру событием SpotPairs (pairs.rs).
//! Кто вызывает: subs.rs (подписка из контракта и пересподписка при смене справочника пар).
//! От чего зависит: ws.rs, symbols.rs, pairs.rs, book.rs, ctx.rs, depth.rs (допустимые уровни и интервалы).
//! Настройки: venue.ws_symbols_per_conn, venue.depth_levels, venue.book_interval.
//!
//! Официально (Spot WebSocket v4, канал spot.order_book, https://www.gate.com/docs/developers/apiv4/ws/en/): одна
//! подписка — одна пара (payload [пара, уровни, интервал]); уровни 5, 10, 20, 50, 100; интервал 100ms или 1000ms.
//! Предела подписок на соединение, числа соединений с IP и частоты запросов в документации канала нет — сколько пар на
//! соединение и подписок в секунду, задаёт Тигран (venue.ws_symbols_per_conn, venue.ws_subs_per_sec), вопрос в
//! docs/TASKS.md; замер нагрузки — до торговли (ADAPTER_TEMPLATE, пункт 8).

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::core::types::{Asset, VenueId};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::book::Books;
use super::ctx::Ctx;
use super::depth::{BOOK_INTERVALS, DEPTH_LEVELS};
use super::pairs::Snapshot;
use super::symbols::Symbols;
use super::ws;

/// Подписывается на стаканы монет. Монеты без торгуемой пары Gate пропускаются с записью в журнал.
/// Возвращает: задачи соединений, снимок пар (pairs.rs) и символы подписанных пар (обновление комиссий — subs.rs).
/// Ошибки: NotConfigured (настройки вне пределов документации или не заданы), справочник пар.
pub async fn subscribe(
    ctx: &Ctx,
    symbols: &Symbols,
    books: &Books,
    assets: Vec<Asset>,
    out: mpsc::Sender<MarketEvent>,
) -> GatewayResult<(Vec<JoinHandle<()>>, Snapshot, Vec<String>)> {
    let per_conn = ctx.int("venue.ws_symbols_per_conn")?.max(1);
    let levels = ctx.int("venue.depth_levels")?;
    if !DEPTH_LEVELS.contains(&levels) {
        let m = format!(
            "venue.depth_levels = {levels}: spot.order_book Gate — только {DEPTH_LEVELS:?}"
        );
        return Err(bad(ctx, m));
    }
    let interval = ctx.text("venue.book_interval")?;
    if !BOOK_INTERVALS.contains(&interval.as_str()) {
        let m = format!(
            "venue.book_interval = {interval}: spot.order_book Gate — только {BOOK_INTERVALS:?}"
        );
        return Err(bad(ctx, m));
    }
    symbols.ensure_fresh(ctx).await?;
    let snap = Snapshot::build(symbols, &assets);
    note(ctx, &snap).await;
    // Ядру — состояние пар и монеты, которыми можно торговать через API: по ним ядро ставит причины отсева и
    // подписывает DEX (core/subscribe.rs). Шлётся при каждой подписке, в том числе пустой.
    let loaded_at = symbols
        .loaded_at()
        .ok_or_else(|| bad(ctx, "справочник пар Gate не получен".to_string()))?;
    let _ = out
        .send(snap.event(VenueId(super::ctx::ID.to_string()), loaded_at))
        .await;
    let ok = snap.books.clone();
    // Решение 202: прежние соединения остановлены (subs.rs) — сторож видит только соединения этой подписки.
    books.feed.clear();
    let mut tasks = Vec::new();
    for (i, chunk) in ok.chunks(per_conn as usize).enumerate() {
        let name = format!("market-{}", i + 1);
        tasks.push(tokio::spawn(ws::run(
            ctx.clone(),
            name,
            chunk.to_vec(),
            books.clone(),
            out.clone(),
        )));
    }
    let syms: Vec<String> = ok.iter().map(|(_, s)| s.clone()).collect();
    Ok((tasks, snap, syms))
}

/// Запись в журнал: монеты без торгуемой пары и пары, закрытые для API. Ошибок нет.
async fn note(ctx: &Ctx, snap: &Snapshot) {
    let mut lines = vec![];
    if !snap.missing.is_empty() {
        lines.push(format!(
            "Gate: нет торгуемой пары для {} монет: {}",
            snap.missing.len(),
            snap.missing.join(", ")
        ));
    }
    let closed = snap.api_closed();
    if !closed.is_empty() {
        lines.push(format!(
            "Gate: пара не торгуется через API (нет в белом списке пар ключа или стороны ещё не открыты) у {} монет: {}",
            closed.len(),
            closed.join(", ")
        ));
    }
    for m in lines {
        let _ = ctx
            .journal
            .write(
                Level::Warn,
                "venue.subscribe",
                Some(super::ctx::ID),
                m,
                None,
            )
            .await;
    }
}

/// Ошибка настройки.
fn bad(ctx: &Ctx, msg: String) -> GatewayError {
    ctx.tag(GatewayError::new(ErrorCode::NotConfigured, None, msg))
}
