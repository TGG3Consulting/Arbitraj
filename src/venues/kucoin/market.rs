//! Файл: venues/kucoin/market.rs
//! Что делает: подписка на стаканы KuCoin по списку монет: делит монеты на соединения (venue.ws_symbols_per_conn монет
//! на соединение) и запускает по задаче на соединение. Состояние пар монет сообщает ядру событием SpotPairs (pairs.rs) —
//! при каждой подписке, в том числе пустой.
//! Кто вызывает: subs.rs (подписка из контракта и пересподписка при смене справочника пар).
//! От чего зависит: ws.rs, ws_conn.rs (пределы), symbols.rs, pairs.rs, book.rs, ctx.rs.
//! Настройки: venue.ws_symbols_per_conn [kucoin].
//!
//! Официально (rate-limit-rule-classic): «Spot/Margin is limited to 400» тем на соединение, «a topic supports up to 100
//! symbols» — не больше 40 000 символов на соединение; «Up to 800» соединений (публичные — по IP).

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
use super::ws_conn::{SYMBOLS_PER_TOPIC, TOPICS_PER_CONN};

/// Подписывается на стаканы монет. Монеты без торгуемой пары KuCoin пропускаются с записью в журнал.
/// Возвращает: задачи соединений, снимок пар и символы подписанных пар (для обновления комиссий).
/// Ошибки: NotConfigured (настройки), справочник пар.
pub async fn subscribe(
    ctx: &Ctx,
    symbols: &Symbols,
    books: &Books,
    assets: Vec<Asset>,
    out: mpsc::Sender<MarketEvent>,
) -> GatewayResult<(Vec<JoinHandle<()>>, Snapshot, Vec<String>)> {
    let per_conn = ctx.int("venue.ws_symbols_per_conn")? as usize;
    let max = SYMBOLS_PER_TOPIC * TOPICS_PER_CONN;
    if per_conn == 0 || per_conn > max {
        return Err(bad(
            ctx,
            format!(
                "venue.ws_symbols_per_conn = {per_conn}: у KuCoin от 1 до {max} (400 тем × 100 символов)"
            ),
        ));
    }
    // Настройки соединения проверяются до запуска: ошибка видна сразу, а не в журнале каждого соединения.
    ws::config(ctx).map_err(|m| bad(ctx, m))?;
    symbols.ensure_fresh(ctx).await?;
    let snap = Snapshot::build(symbols, &assets);
    if !snap.missing.is_empty() {
        let m = format!(
            "KuCoin: нет торгуемой пары для {} монет: {}",
            snap.missing.len(),
            snap.missing.join(", ")
        );
        ctx.log(Level::Warn, "venue.subscribe", m).await;
    }
    let loaded_at = symbols
        .loaded_at()
        .ok_or_else(|| bad(ctx, "справочник пар KuCoin не получен".to_string()))?;
    let _ = out
        .send(snap.event(VenueId(super::ctx::ID.to_string()), loaded_at))
        .await;
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
