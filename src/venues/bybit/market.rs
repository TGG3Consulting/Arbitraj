//! Файл: venues/bybit/market.rs
//! Что делает: подписка на стаканы Bybit по списку монет: делит монеты на соединения
//! (venue.ws_symbols_per_conn монет на соединение) и запускает по задаче на соединение.
//! Состояние пар монет и монеты, которыми можно торговать через API, сообщает ядру событием SpotPairs (pairs.rs).
//! Кто вызывает: subs.rs (подписка из контракта и пересподписка при смене справочника пар).
//! От чего зависит: ws.rs, symbols.rs, pairs.rs, book.rs, ctx.rs.
//! Настройки: venue.ws_symbols_per_conn, venue.depth_levels.
//!
//! Официально (Connect https://bybit-exchange.github.io/docs/v5/ws/connect, «Args limits»): «Spot can input up to 100
//! args for a single connection (effecive from 26 Nov, 2026)», «Spot can input up to 10 args for each subscription
//! request», «you cannot have length of "args" array over 21,000 characters» на соединение; «Do not build over 500
//! connections in 5 minutes». Глубины спота (Orderbook): 1, 50, 200, 1000. Одна монета — одна подписка (стакан).

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

/// Предел подписок на одно соединение — из официальной документации Bybit.
const MAX_TOPICS_PER_CONN: u64 = 100;
/// Допустимые глубины стакана спота — из официальной документации Bybit (Orderbook → Depths → Spot).
pub const DEPTH_LEVELS: [u64; 4] = [1, 50, 200, 1000];
/// Наибольшая длина args на одно соединение, символов — из официальной документации («over 21,000 characters»).
const MAX_ARGS_CHARS: usize = 21_000;

/// Подписывается на стаканы монет. Монеты, которых нет среди торгуемых пар Bybit, пропускаются
/// с записью в журнал. Возвращает: запущенные задачи соединений и символы подписанных пар (по ним subs.rs ведёт
/// обновление комиссий) —
/// вызывающий хранит их и останавливает перед новой подпиской, — и снимок пар (pairs.rs), с которым
/// pairs_watch.rs сравнивает обновлённый справочник пар.
/// Ошибки: NotConfigured (настройки вне пределов документации или не заданы), справочник пар.
pub async fn subscribe(
    ctx: &Ctx,
    symbols: &Symbols,
    books: &Books,
    assets: Vec<Asset>,
    out: mpsc::Sender<MarketEvent>,
) -> GatewayResult<(Vec<JoinHandle<()>>, Snapshot, Vec<String>)> {
    let per_conn = ctx.int("venue.ws_symbols_per_conn")?;
    if per_conn > MAX_TOPICS_PER_CONN {
        return Err(bad(
            ctx,
            format!(
                "venue.ws_symbols_per_conn = {per_conn}: у Bybit не больше {MAX_TOPICS_PER_CONN} подписок на соединение"
            ),
        ));
    }
    let levels = ctx.int("venue.depth_levels")?;
    if !DEPTH_LEVELS.contains(&levels) {
        return Err(bad(
            ctx,
            format!("venue.depth_levels = {levels}: Bybit поддерживает только {DEPTH_LEVELS:?}"),
        ));
    }
    symbols.ensure_fresh(ctx).await?;
    let snap = Snapshot::build(symbols, &assets);
    note(ctx, &snap).await;
    // Ядру — состояние пар и монеты, которыми можно торговать через API (pairs.rs): по ним ядро ставит причины отсева
    // и подписывает DEX (core/subscribe.rs). Шлётся при каждой подписке, в том числе пустой (площадка выключена — пар нет).
    let loaded_at = symbols
        .loaded_at()
        .ok_or_else(|| bad(ctx, "справочник пар Bybit не получен".to_string()))?;
    let _ = out
        .send(snap.event(VenueId(super::ctx::ID.to_string()), loaded_at))
        .await;
    let ok = snap.books.clone();
    let chunks = split(&ok, per_conn as usize, levels);
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
    // Ставки комиссии подписанных пар обновляются фоном (котировка берёт их из памяти). Задачу обновления держит
    // subs.rs: при том же списке символов круг fee-rate не начинается заново (решение 131).
    let syms: Vec<String> = ok.iter().map(|(_, s)| s.clone()).collect();
    Ok((tasks, snap, syms))
}

/// Запись в журнал: монеты без торгуемой пары и пары, закрытые для API. Ошибок нет.
async fn note(ctx: &Ctx, snap: &Snapshot) {
    let mut lines = vec![];
    if !snap.missing.is_empty() {
        lines.push(format!(
            "Bybit: нет торгуемой пары для {} монет: {}",
            snap.missing.len(),
            snap.missing.join(", ")
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

/// Делит монеты на соединения: не больше per_conn подписок и не больше MAX_ARGS_CHARS символов args на соединение.
/// Принимает: монеты (монета, символ), предел на соединение, глубину. Возвращает: пачки. Ошибок нет.
pub fn split(list: &[(Asset, String)], per_conn: usize, levels: u64) -> Vec<Vec<(Asset, String)>> {
    let mut out: Vec<Vec<(Asset, String)>> = vec![];
    let mut chars = 0usize;
    for item in list {
        // Длина элемента в массиве args: кавычки и запятая.
        let len = super::depth::channel(levels, &item.1).len() + 3;
        match out.last_mut() {
            Some(cur) if cur.len() < per_conn.max(1) && chars + len <= MAX_ARGS_CHARS => {
                cur.push(item.clone());
                chars += len;
            }
            _ => {
                out.push(vec![item.clone()]);
                chars = len;
            }
        }
    }
    out
}

/// Ошибка настройки.
fn bad(ctx: &Ctx, msg: String) -> GatewayError {
    ctx.tag(GatewayError::new(ErrorCode::NotConfigured, None, msg))
}
