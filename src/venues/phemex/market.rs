//! Файл: venues/phemex/market.rs
//! Что делает: подписка на стаканы Phemex по списку монет: делит монеты на соединения (venue.ws_symbols_per_conn монет
//! на соединение, не больше 20) и запускает по задаче на соединение, не больше venue.ws_max_conns соединений (вместе с
//! потоком аккаунта, если он включён). Монеты сверх этого потолка не подписываются — запись в журнал (котировок по ним
//! нет). Состояние пар монет сообщает ядру событием SpotPairs (pairs.rs) — при каждой подписке, в том числе пустой.
//! Кто вызывает: subs.rs (подписка из контракта и пересподписка при смене справочника пар).
//! От чего зависит: ws.rs, symbols.rs, pairs.rs, book.rs, ctx.rs.
//! Настройки: venue.ws_symbols_per_conn, venue.ws_max_conns, venue.balance_events [phemex].
//!
//! Официально (Websocket API Standards → API Rate Limits, старая официальная документация github.com/phemex/
//! phemex-api-docs Public-Spot-API-en.md; в текущей — тот же текст в разделе контрактов, https://phemex-docs.github.io/
//! #api-rate-limits): «Each Client has concurrent connection limit to 5 in maximum», «Each connection has subscription
//! limit to 20 in maximum», «Each connection has throttle limit to 20 request/s». Что такое «Client» (IP, ключ, UID) —
//! не сказано: потолок соединений — настройка (вопрос Тиграну, проверить на живом). Итого не больше 100 стаканов.

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

/// Подписок на соединение — «subscription limit to 20 in maximum».
pub const SUBS_PER_CONN: u64 = 20;
/// Соединений на клиента — «concurrent connection limit to 5 in maximum».
pub const CONNS_MAX: u64 = 5;

/// Подписывается на стаканы монет. Монеты без торгуемой пары и сверх потолка соединений — в журнал.
/// Возвращает: задачи соединений, снимок пар и символы подписанных пар (для обновления комиссий).
/// Ошибки: NotConfigured (настройки вне пределов документации), справочник пар.
pub async fn subscribe(
    ctx: &Ctx,
    symbols: &Symbols,
    books: &Books,
    assets: Vec<Asset>,
    out: mpsc::Sender<MarketEvent>,
) -> GatewayResult<(Vec<JoinHandle<()>>, Snapshot, Vec<String>)> {
    let per_conn = ctx.int("venue.ws_symbols_per_conn")?;
    if per_conn == 0 || per_conn > SUBS_PER_CONN {
        return Err(bad(
            ctx,
            format!(
                "venue.ws_symbols_per_conn = {per_conn}: у Phemex от 1 до {SUBS_PER_CONN} подписок на соединение"
            ),
        ));
    }
    let conns = ctx.int("venue.ws_max_conns")?;
    if conns == 0 || conns > CONNS_MAX {
        return Err(bad(
            ctx,
            format!("venue.ws_max_conns = {conns}: у Phemex от 1 до {CONNS_MAX} соединений"),
        ));
    }
    // Поток аккаунта — тоже соединение на тот же адрес.
    let private = u64::from(ctx.flag("venue.balance_events").unwrap_or(false));
    let book_conns = conns.saturating_sub(private) as usize;
    // Настройки соединения проверяются до запуска: ошибка видна сразу, а не в журнале каждого соединения.
    ws::config(ctx).map_err(|m| bad(ctx, m))?;
    symbols.ensure_fresh(ctx).await?;
    let snap = Snapshot::build(symbols, &assets);
    if !snap.missing.is_empty() {
        let m = format!(
            "Phemex: нет торгуемой пары для {} монет: {}",
            snap.missing.len(),
            snap.missing.join(", ")
        );
        ctx.log(Level::Warn, "venue.subscribe", m).await;
    }
    let loaded_at = symbols
        .loaded_at()
        .ok_or_else(|| bad(ctx, "справочник пар Phemex не получен".to_string()))?;
    let _ = out
        .send(snap.event(VenueId(super::ctx::ID.to_string()), loaded_at))
        .await;
    let cap = book_conns * per_conn as usize;
    if snap.books.len() > cap {
        let over: Vec<&str> = snap.books[cap..]
            .iter()
            .map(|(a, _)| a.ticker.as_str())
            .collect();
        let m = format!(
            "Phemex: стаканов больше потолка ({book_conns} соединений × {per_conn}; поток аккаунта — {private}) — \
             не подписаны {} монет, котировок по ним нет: {}",
            over.len(),
            over.join(", ")
        );
        ctx.log(Level::Warn, "venue.subscribe", m).await;
    }
    let taken = &snap.books[..snap.books.len().min(cap)];
    // Решение 202: прежние соединения остановлены (subs.rs) — сторож видит только соединения этой подписки.
    books.feed.clear();
    let mut tasks = Vec::new();
    for (i, chunk) in taken.chunks(per_conn as usize).enumerate() {
        tasks.push(tokio::spawn(ws::run(
            ctx.clone(),
            format!("market-{}", i + 1),
            chunk.to_vec(),
            symbols.clone(),
            books.clone(),
            out.clone(),
        )));
    }
    let syms: Vec<String> = taken.iter().map(|(_, s)| s.clone()).collect();
    Ok((tasks, snap, syms))
}

/// Ошибка настройки.
fn bad(ctx: &Ctx, msg: String) -> GatewayError {
    ctx.tag(GatewayError::new(ErrorCode::NotConfigured, None, msg))
}
