//! Файл: venues/binance/market.rs
//! Что делает: подписка на стаканы Binance по списку монет: делит монеты на соединения (venue.ws_symbols_per_conn на
//! соединение, не больше 1024 потоков по документации) и запускает задачу на соединение; ядру — событие SpotPairs.
//! Кто вызывает: subs.rs (подписка из контракта и пересподписка при смене справочника пар).
//! От чего зависит: ws.rs, symbols.rs, pairs.rs, book.rs, ctx.rs.
//! Настройки: venue.ws_symbols_per_conn (и всё, что читает ws.rs::config — проверяется до запуска соединений).
//!
//! Официально (web-socket-streams «WebSocket Limits»): «A single connection can listen to a maximum of 1024 streams»;
//! «There is a limit of 300 connections per attempt every 5 minutes per IP» — соединения открываются задачами сразу,
//! поэтому монет на соединение держать большим (сотни), чтобы соединений было мало.

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::core::types::{Asset, VenueId};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::book::Books;
use super::ctx::Ctx;
use super::pairs::Snapshot;
use super::symbols::Symbols;
use super::ws;

/// Предел потоков на соединение — из официальной документации.
const MAX_STREAMS_PER_CONN: u64 = 1024;

/// Подписывается на стаканы монет. Возвращает: задачи соединений, снимок пар и символы подписанных пар (по ним
/// subs.rs ведёт обновление комиссий). Ошибки: NotConfigured (настройки вне пределов или не заданы), справочник пар.
pub async fn subscribe(
    ctx: &Ctx,
    symbols: &Symbols,
    books: &Books,
    assets: Vec<Asset>,
    out: mpsc::Sender<MarketEvent>,
) -> GatewayResult<(Vec<JoinHandle<()>>, Snapshot, Vec<String>)> {
    let per_conn = ctx.int("venue.ws_symbols_per_conn")?;
    if per_conn == 0 || per_conn > MAX_STREAMS_PER_CONN {
        return Err(bad(
            ctx,
            format!(
                "venue.ws_symbols_per_conn = {per_conn}: у Binance от 1 до {MAX_STREAMS_PER_CONN} потоков на соединение"
            ),
        ));
    }
    // Настройки соединений проверяются сразу: ошибка — подписка не начинается (а не соединения, упавшие по одному).
    if !assets.is_empty() {
        ws::config(ctx).map_err(|m| bad(ctx, m))?;
    }
    symbols.ensure_fresh(ctx).await?;
    let snap = Snapshot::build(symbols, &assets);
    note(ctx, &snap).await;
    let loaded_at = symbols
        .loaded_at()
        .ok_or_else(|| bad(ctx, "справочник пар Binance не получен".to_string()))?;
    let _ = out
        .send(snap.event(VenueId(super::ctx::ID.to_string()), loaded_at))
        .await;
    // Решение 202: прежние соединения остановлены (subs.rs) — сторож видит только соединения этой подписки.
    books.feed.clear();
    let mut tasks = Vec::new();
    for (i, chunk) in snap.books.chunks(per_conn as usize).enumerate() {
        tasks.push(tokio::spawn(ws::run(
            ctx.clone(),
            format!("market-{}", i + 1),
            chunk.to_vec(),
            books.clone(),
            out.clone(),
        )));
    }
    let syms: Vec<String> = snap.books.iter().map(|(_, s)| s.clone()).collect();
    Ok((tasks, snap, syms))
}

/// Запись в журнал: монеты без торгуемой пары и пары, закрытые для API. Ошибок нет.
async fn note(ctx: &Ctx, snap: &Snapshot) {
    if !snap.missing.is_empty() {
        let m = format!(
            "Binance: нет пары TRADING для {} монет: {}",
            snap.missing.len(),
            snap.missing.join(", ")
        );
        ctx.log(Level::Warn, "venue.subscribe", m).await;
    }
    let closed = snap.api_closed();
    if !closed.is_empty() {
        let m = format!(
            "Binance: пара не торгуется через API (isSpotTradingAllowed или права аккаунта permissionSets) у {} монет: {}",
            closed.len(),
            closed.join(", ")
        );
        ctx.log(Level::Warn, "venue.subscribe", m).await;
    }
}

/// Ошибка настройки. Ошибок нет.
fn bad(ctx: &Ctx, msg: String) -> GatewayError {
    ctx.tag(GatewayError::new(ErrorCode::NotConfigured, None, msg))
}
