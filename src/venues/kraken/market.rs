//! Файл: venues/kraken/market.rs
//! Что делает: подписка на стаканы Kraken по списку монет: делит пары на соединения (venue.ws_symbols_per_conn пар на
//! соединение) и запускает по задаче на соединение; подключения к серверу (любые соединения Kraken с этого IP)
//! проходят общий ограничитель попыток ws_req::connect_slot. Состояние пар монет — ядру событием SpotPairs (pairs.rs)
//! при каждой подписке, в том числе пустой.
//! Кто вызывает: subs.rs (подписка из контракта и пересподписка при смене справочника пар).
//! От чего зависит: ws.rs, symbols.rs, pairs.rs, book.rs, ctx.rs.
//! Настройки: venue.ws_symbols_per_conn [kraken].
//!
//! Официально (https://docs.kraken.com/exchange/guides/websockets/introduction.md): соединений — «approximately 150
//! attempts per rolling 10 minutes» на IP, сверх — бан IP на 10 минут (ограничитель — ws_req.rs). Сколько символов в
//! одном соединении — в документации не указано: venue.ws_symbols_per_conn подбирается замером.

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

/// Пара соединения: монета, имя WebSocket и знаки цены/количества (формат чек-суммы стакана).
#[derive(Debug, Clone)]
pub struct Sub {
    pub asset: Asset,
    pub wsname: String,
    pub price_dp: u32,
    pub lot_dp: u32,
}

/// Подписывается на стаканы монет. Монеты без торгуемой пары Kraken пропускаются с записью в журнал.
/// Возвращает: задачи соединений, снимок пар и имена пар REST (для обновления комиссий).
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
        return Err(bad(ctx, "venue.ws_symbols_per_conn = 0".into()));
    }
    // Настройки соединения проверяются до запуска: ошибка видна сразу, а не в журнале каждого соединения.
    ws::config(ctx).map_err(|m| bad(ctx, m))?;
    symbols.ensure_fresh(ctx).await?;
    let snap = Snapshot::build(symbols, &assets);
    if !snap.missing.is_empty() {
        let m = format!(
            "Kraken: нет торгуемой пары для {} монет: {}",
            snap.missing.len(),
            snap.missing.join(", ")
        );
        ctx.log(Level::Warn, "venue.subscribe", m).await;
    }
    let loaded_at = symbols
        .loaded_at()
        .ok_or_else(|| bad(ctx, "справочник пар Kraken не получен".to_string()))?;
    let _ = out
        .send(snap.event(VenueId(super::ctx::ID.to_string()), loaded_at))
        .await;
    let all = symbols.all();
    let subs: Vec<Sub> = snap
        .books
        .iter()
        .filter_map(|(a, _)| {
            all.get(&a.ticker).map(|r| Sub {
                asset: a.clone(),
                wsname: r.wsname.clone(),
                price_dp: r.price_decimals,
                lot_dp: r.lot_decimals,
            })
        })
        .collect();
    let mut tasks = Vec::new();
    for (i, chunk) in subs.chunks(per_conn).enumerate() {
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
