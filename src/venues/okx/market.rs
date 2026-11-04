//! Файл: venues/okx/market.rs
//! Что делает: подписка на стаканы OKX по списку монет: делит монеты на соединения (venue.ws_symbols_per_conn монет на
//! соединение) и запускает по задаче на соединение; соединения открываются не чаще venue.ws_connects_per_sec в секунду.
//! Состояние пар монет сообщает ядру событием SpotPairs (pairs.rs) — при каждой подписке, в том числе пустой.
//! Кто вызывает: subs.rs (подписка из контракта и пересподписка при смене справочника пар).
//! От чего зависит: ws.rs, symbols.rs, pairs.rs, book.rs, ctx.rs.
//! Настройки: venue.ws_symbols_per_conn, venue.ws_connects_per_sec [okx].
//!
//! Официально (https://www.okx.com/docs-v5/en/, «Overview → WebSocket → Connect»): «Connection limit: 3 requests per
//! second (based on IP)»; «The limit will be set at 30 WebSocket connections per specific WebSocket channel per
//! sub-account»; «the total length of multiple channels cannot exceed 64 KB». Числа каналов на одно соединение в
//! документации нет — venue.ws_symbols_per_conn подбирается по живому отказу (коды закрытия 4008/4009).

use std::time::Duration;

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

/// Подключений в секунду с одного IP — официально «3 requests per second (based on IP)».
pub const MAX_CONNECTS_PER_SEC: u64 = 3;
/// Соединений на канал на субсчёт — официально «30 WebSocket connections per specific WebSocket channel».
pub const MAX_CONNS_PER_CHANNEL: usize = 30;

/// Подписывается на стаканы монет. Монеты без торгуемой пары OKX пропускаются с записью в журнал.
/// Возвращает: задачи соединений, снимок пар и instId подписанных пар (для обновления комиссий).
/// Ошибки: NotConfigured (настройки), справочник пар.
pub async fn subscribe(
    ctx: &Ctx,
    symbols: &Symbols,
    books: &Books,
    assets: Vec<Asset>,
    out: mpsc::Sender<MarketEvent>,
) -> GatewayResult<(Vec<JoinHandle<()>>, Snapshot, Vec<String>)> {
    let per_conn = ctx.int("venue.ws_symbols_per_conn")? as usize;
    let per_sec = ctx.int("venue.ws_connects_per_sec")?;
    if per_conn == 0 {
        return Err(bad(ctx, "venue.ws_symbols_per_conn = 0".into()));
    }
    if per_sec == 0 || per_sec > MAX_CONNECTS_PER_SEC {
        return Err(bad(
            ctx,
            format!(
                "venue.ws_connects_per_sec = {per_sec}: у OKX от 1 до {MAX_CONNECTS_PER_SEC} подключений в секунду"
            ),
        ));
    }
    // Настройки соединения проверяются до запуска: ошибка видна сразу, а не в журнале каждого соединения.
    ws::config(ctx).map_err(|m| bad(ctx, m))?;
    symbols.ensure_fresh(ctx).await?;
    let snap = Snapshot::build(symbols, &assets);
    let conns = snap.books.len().div_ceil(per_conn);
    if conns > MAX_CONNS_PER_CHANNEL {
        return Err(bad(
            ctx,
            format!(
                "OKX: {} стаканов при venue.ws_symbols_per_conn = {per_conn} — {conns} соединений, а на канал не больше \
                 {MAX_CONNS_PER_CHANNEL}",
                snap.books.len()
            ),
        ));
    }
    if !snap.missing.is_empty() {
        let m = format!(
            "OKX: нет торгуемой пары для {} монет: {}",
            snap.missing.len(),
            snap.missing.join(", ")
        );
        ctx.log(Level::Warn, "venue.subscribe", m).await;
    }
    let loaded_at = symbols
        .loaded_at()
        .ok_or_else(|| bad(ctx, "справочник пар OKX не получен".to_string()))?;
    let _ = out
        .send(snap.event(VenueId(super::ctx::ID.to_string()), loaded_at))
        .await;
    let gap = Duration::from_secs_f64(1.0 / per_sec as f64);
    let mut tasks = Vec::new();
    for (i, chunk) in snap.books.chunks(per_conn).enumerate() {
        tasks.push(tokio::spawn(ws::run(
            ctx.clone(),
            format!("market-{}", i + 1),
            chunk.to_vec(),
            books.clone(),
            out.clone(),
            gap * i as u32,
        )));
    }
    let syms: Vec<String> = snap.books.iter().map(|(_, s)| s.clone()).collect();
    Ok((tasks, snap, syms))
}

/// Ошибка настройки.
fn bad(ctx: &Ctx, msg: String) -> GatewayError {
    ctx.tag(GatewayError::new(ErrorCode::NotConfigured, None, msg))
}
