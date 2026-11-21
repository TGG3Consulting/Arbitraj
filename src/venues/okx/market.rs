//! Файл: venues/okx/market.rs
//! Что делает: подписка на стаканы OKX по списку монет: делит монеты на соединения (venue.ws_symbols_per_conn монет на
//! соединение) и запускает по задаче на соединение; соединения открываются не чаще venue.ws_connects_per_sec в секунду.
//! Состояние пар монет сообщает ядру событием SpotPairs (pairs.rs) — при каждой подписке, в том числе пустой.
//! Решение 429 (страховка стакана): при канале books (venue.depth_levels = 400) на те же группы пар открываются ещё
//! столько же страховочных соединений «backup-N» с каналом books5 — у каждого соединения свой предел «480 times per
//! hour» запросов подписки; подключения обоих видов идут одной очередью темпа venue.ws_connects_per_sec (предел IP «3
//! requests per second» общий); предел «30 WebSocket connections per specific WebSocket channel» проверяется по каналу
//! books, у books5 соединений столько же. Совместная подписка books и books5 на одну пару документацией не запрещена
//! (запрещена только «books-l2-tbt and books50-l2-tbt/books channels for the same trading symbol»).
//! Кто вызывает: subs.rs (подписка из контракта и пересподписка при смене справочника пар).
//! От чего зависит: ws.rs, backup.rs, symbols.rs, pairs.rs, book.rs, ctx.rs.
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

use super::backup::Role;
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
    let main = ws::config(ctx, Role::Main).map_err(|m| bad(ctx, m))?;
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
    // Решение 202: прежние соединения остановлены (subs.rs) — сторож видит только соединения этой подписки.
    books.feed.clear();
    books.backup.clear();
    // Решение 429: страховочные соединения books5 — только при быстром канале books.
    let mut roles = vec![(Role::Main, "market")];
    if main.channel == "books" {
        roles.push((Role::Backup, "backup"));
    }
    let mut tasks = Vec::new();
    let mut n = 0u32;
    for (role, prefix) in roles {
        for (i, chunk) in snap.books.chunks(per_conn).enumerate() {
            tasks.push(tokio::spawn(ws::run(
                ctx.clone(),
                format!("{prefix}-{}", i + 1),
                chunk.to_vec(),
                books.clone(),
                out.clone(),
                gap * n,
                role,
            )));
            n += 1;
        }
    }
    let syms: Vec<String> = snap.books.iter().map(|(_, s)| s.clone()).collect();
    Ok((tasks, snap, syms))
}

/// Ошибка настройки.
fn bad(ctx: &Ctx, msg: String) -> GatewayError {
    ctx.tag(GatewayError::new(ErrorCode::NotConfigured, None, msg))
}
