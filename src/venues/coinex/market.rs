//! Файл: venues/coinex/market.rs
//! Что делает: подписка на стаканы CoinEx по списку монет: делит монеты на соединения (venue.ws_symbols_per_conn монет
//! на соединение) и запускает по задаче на соединение. Состояние пар монет сообщает ядру событием SpotPairs (pairs.rs) —
//! при каждой подписке, в том числе пустой.
//! Кто вызывает: subs.rs (подписка из контракта и пересподписка при смене справочника пар).
//! От чего зависит: ws.rs, ws_conn.rs (пределы), symbols.rs, pairs.rs, book.rs, ctx.rs.
//! Настройки: venue.ws_symbols_per_conn [coinex].
//!
//! Официально: предела рынков на соединение, сообщений в секунду и числа соединений в документации CoinEx нет (Spot →
//! Market → WS Market Depth https://docs.coinex.com/api/v2/spot/market/ws/market-depth; есть только ошибка 23002 «Requests
//! submitted too frequently», https://docs.coinex.com/api/v2/error) — сколько рынков на соединение, решает Тигран по
//! живому подключению (venue.ws_symbols_per_conn, без предела в коде).

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

/// Подписывается на стаканы монет. Монеты без торгуемой пары CoinEx пропускаются с записью в журнал.
/// Возвращает: задачи соединений, снимок пар и рынки подписанных пар (для обновления комиссий).
/// Ошибки: NotConfigured (настройки), справочник пар.
pub async fn subscribe(
    ctx: &Ctx,
    symbols: &Symbols,
    books: &Books,
    assets: Vec<Asset>,
    out: mpsc::Sender<MarketEvent>,
) -> GatewayResult<(Vec<JoinHandle<()>>, Snapshot, Vec<String>)> {
    let per_conn = ctx.int("venue.ws_symbols_per_conn")? as usize;
    if per_conn == 0 {
        return Err(bad(
            ctx,
            "venue.ws_symbols_per_conn = 0: нужен хотя бы один рынок на соединение".to_string(),
        ));
    }
    // Настройки соединения проверяются до запуска: ошибка видна сразу, а не в журнале каждого соединения.
    ws::config(ctx).map_err(|m| bad(ctx, m))?;
    symbols.ensure_fresh(ctx).await?;
    let snap = Snapshot::build(symbols, &assets);
    if !snap.missing.is_empty() {
        let m = format!(
            "CoinEx: нет торгуемой пары для {} монет: {}",
            snap.missing.len(),
            snap.missing.join(", ")
        );
        ctx.log(Level::Warn, "venue.subscribe", m).await;
    }
    let loaded_at = symbols
        .loaded_at()
        .ok_or_else(|| bad(ctx, "справочник пар CoinEx не получен".to_string()))?;
    let _ = out
        .send(snap.event(VenueId(super::ctx::ID.to_string()), loaded_at))
        .await;
    // Решение 202: прежние соединения остановлены (subs.rs) — сторож видит только соединения этой подписки.
    books.feed.clear();
    let mut tasks = Vec::new();
    for (i, chunk) in snap.books.chunks(per_conn).enumerate() {
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

/// Ошибка настройки.
fn bad(ctx: &Ctx, msg: String) -> GatewayError {
    ctx.tag(GatewayError::new(ErrorCode::NotConfigured, None, msg))
}
