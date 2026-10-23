//! Файл: venues/bingx/market.rs
//! Что делает: подписка на стаканы BingX по списку монет: делит монеты на соединения
//! (venue.ws_symbols_per_conn монет на соединение) и запускает по задаче на соединение.
//! Кто вызывает: mod.rs (subscribe_market из контракта).
//! От чего зависит: ws.rs, symbols.rs, book.rs, ctx.rs.
//! Настройки: venue.ws_symbols_per_conn, venue.depth_levels.
//!
//! Официально: не больше 200 подписок на соединение, превышение — код 80403 (раздел WebSocket docs-v3;
//! для спота то же — объявление BingX по споту, см. research/flow/bingx_ws.md); уровни стакана спота:
//! 5, 10, 20, 50, 100 (api-ai-skills/spot-ws-market). Лимита соединений с IP для спота FAQ не задаёт.

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::core::types::Asset;
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::book::Books;
use super::ctx::Ctx;
use super::fees::Fees;
use super::symbols::Symbols;
use super::ws;

/// Предел подписок на одно соединение — из официальной документации BingX.
const MAX_TOPICS_PER_CONN: u64 = 200;
/// Допустимые уровни стакана канала depth — из официальной документации BingX.
const DEPTH_LEVELS: [u64; 5] = [5, 10, 20, 50, 100];

/// Подписывается на стаканы монет. Монеты, которых нет среди торгуемых пар BingX, пропускаются
/// с записью в журнал. Возвращает: запущенные задачи (соединения и обновление комиссий) —
/// вызывающий хранит их и останавливает перед новой подпиской.
/// Ошибки: NotConfigured (настройки вне пределов документации или не заданы), справочник пар.
pub async fn subscribe(
    ctx: &Ctx,
    symbols: &Symbols,
    books: &Books,
    fees: &Fees,
    assets: Vec<Asset>,
    out: mpsc::Sender<MarketEvent>,
) -> GatewayResult<Vec<JoinHandle<()>>> {
    let per_conn = ctx.int("venue.ws_symbols_per_conn")?;
    if per_conn > MAX_TOPICS_PER_CONN {
        return Err(bad(
            ctx,
            format!(
                "venue.ws_symbols_per_conn = {per_conn}: у BingX не больше {MAX_TOPICS_PER_CONN} подписок на соединение"
            ),
        ));
    }
    let levels = ctx.int("venue.depth_levels")?;
    if !DEPTH_LEVELS.contains(&levels) {
        return Err(bad(
            ctx,
            format!("venue.depth_levels = {levels}: BingX поддерживает только {DEPTH_LEVELS:?}"),
        ));
    }
    symbols.ensure_fresh(ctx).await?;
    let mut ok = Vec::new();
    let mut missing = Vec::new();
    for a in assets {
        match symbols.get(ctx, &a.ticker) {
            Ok(r) if r.status == super::symbols::STATUS_ONLINE => ok.push((a, r.symbol)),
            _ => missing.push(a.ticker),
        }
    }
    if !missing.is_empty() {
        let m = format!(
            "BingX: нет торгуемой пары для {} монет: {}",
            missing.len(),
            missing.join(", ")
        );
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
    let chunks: Vec<Vec<(Asset, String)>> =
        ok.chunks(per_conn as usize).map(|c| c.to_vec()).collect();
    let mut tasks = Vec::new();
    for (i, chunk) in chunks.iter().enumerate() {
        let name = format!("market-{}", i + 1);
        tasks.push(tokio::spawn(ws::run(
            ctx.clone(),
            name,
            chunk.clone(),
            books.clone(),
            out.clone(),
        )));
    }
    // Ставки комиссии подписанных пар обновляются фоном (котировка берёт их из памяти).
    let syms: Vec<String> = ok.iter().map(|(_, s)| s.clone()).collect();
    tasks.push(tokio::spawn(fees.clone().refresh_loop(ctx.clone(), syms)));
    Ok(tasks)
}

/// Ошибка настройки.
fn bad(ctx: &Ctx, msg: String) -> GatewayError {
    ctx.tag(GatewayError::new(ErrorCode::NotConfigured, None, msg))
}
