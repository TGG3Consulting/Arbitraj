//! Файл: venues/xt/market.rs
//! Что делает: подписка на стаканы XT.com по списку монет: делит монеты на соединения (venue.ws_symbols_per_conn на
//! соединение) и запускает задачу на соединение; ядру — событие SpotPairs.
//! Кто вызывает: subs.rs (подписка из контракта и пересподписка при смене справочника пар).
//! От чего зависит: ws.rs, symbols.rs, pairs.rs, book.rs, ctx.rs.
//! Настройки: venue.ws_symbols_per_conn (и всё, что читает ws.rs::config — проверяется до запуска соединений);
//! venue.order_time_in_force — для признака пары «XT не примет наш ордер» (решение 254, pairs.rs).
//!
//! Официально (https://doc.xt.com/#websocket_publicbase): предел тем на соединение и соединений с IP в документации
//! XT НЕ указан — venue.ws_symbols_per_conn подбирается живым замером нагрузки (решение 109), вопрос Тиграну.

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

/// Подписывается на стаканы монет. Возвращает: задачи соединений и снимок пар (ставка комиссии у XT — в справочнике
/// пар, фонового круга нет). Ошибки: NotConfigured (настройки вне пределов или не заданы), справочник пар.
pub async fn subscribe(
    ctx: &Ctx,
    symbols: &Symbols,
    books: &Books,
    assets: Vec<Asset>,
    out: mpsc::Sender<MarketEvent>,
) -> GatewayResult<(Vec<JoinHandle<()>>, Snapshot)> {
    let per_conn = ctx.int("venue.ws_symbols_per_conn")?;
    if per_conn == 0 {
        return Err(bad(
            ctx,
            "venue.ws_symbols_per_conn = 0: нужна хотя бы одна монета на соединение".to_string(),
        ));
    }
    // Настройки соединений проверяются сразу: ошибка — подписка не начинается (а не соединения, упавшие по одному).
    if !assets.is_empty() {
        ws::config(ctx).map_err(|m| bad(ctx, m))?;
    }
    symbols.ensure_fresh(ctx).await?;
    let tif = ctx.text("venue.order_time_in_force").ok();
    let snap = Snapshot::build(symbols, &assets, tif.as_deref());
    note(ctx, &snap).await;
    let loaded_at = symbols
        .loaded_at()
        .ok_or_else(|| bad(ctx, "справочник пар XT не получен".to_string()))?;
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
    Ok((tasks, snap))
}

/// Запись в журнал: монеты без торгуемой пары, пары, где XT не примет наш ордер (решение 254), и пары, закрытые для
/// API. Ошибок нет.
async fn note(ctx: &Ctx, snap: &Snapshot) {
    if !snap.missing.is_empty() {
        let m = format!(
            "XT: нет пары ONLINE для {} монет: {}",
            snap.missing.len(),
            snap.missing.join(", ")
        );
        ctx.log(Level::Warn, "venue.subscribe", m).await;
    }
    if !snap.bad_symbols.is_empty() {
        let m = format!(
            "XT: символ пары с недопустимыми для запроса XT знаками (разрешены буквы, цифры, «_ . , -») у {} пар — стакан не подписан, через API не торгуются: {}",
            snap.bad_symbols.len(),
            snap.bad_symbols.join(", ")
        );
        ctx.log(Level::Warn, "venue.subscribe", m).await;
    }
    let blocked = snap.order_blocked();
    if !blocked.is_empty() {
        let m = format!(
            "XT: биржа не примет наш ордер (LIMIT, venue.order_time_in_force) на {} парах — в расчёт не берутся (решение 254): {}",
            blocked.len(),
            blocked.join("; ")
        );
        ctx.log(Level::Warn, "venue.subscribe", m).await;
    }
    let closed = snap.api_closed();
    if !closed.is_empty() {
        let m = format!(
            "XT: пара не торгуется через API (tradingEnabled или openapiEnabled = false) у {} монет: {}",
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
