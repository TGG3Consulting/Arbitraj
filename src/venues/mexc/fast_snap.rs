//! Файл: venues/mexc/fast_snap.rs
//! Что делает: решение 420 — снимки стакана REST GET /api/v3/depth для быстрых стаканов MEXC (fast_book.rs): одна
//! очередь на все соединения с темпом venue.fast_book_snapshot_per_sec — старт сотен пар и пересинхронизации после
//! переподключения идут по очереди, а не залпом (корзина IP MEXC 300 веса / 10 с, после 429 — бан IP). Перед запросом
//! в канал соединения уходит «запрос ушёл» (с него копятся кадры — fast_book.rs start_buffer), после —
//! итог: lastUpdateId и уровни в числах ядра или причина сбоя. Образец очереди — venues/xt/snapshots.rs.
//! Кто вызывает: fast_pair.rs (запрос снимка пары), ws.rs (проверка настроек при старте сессии).
//! От чего зависит: rest.rs (public: ограничитель веса ip_weight.rs, пауза после 418/429), book_print.rs (levels —
//! тот же разбор уровней ответа), ctx.rs.
//! Настройки: venue.fast_book_snapshot_per_sec [mexc] (1–10), venue.fast_book_snapshot_limit [mexc] (1–5000).
//!
//! Официально: Market Data → Order Book (https://www.mexc.com/api-docs/spot-v3/market-data-endpoints/order-book):
//! «GET /api/v3/depth», «Weight(IP): 3», limit «default 100; max 5000», ответ {lastUpdateId, bids, asks}; «Rate Limits»
//! (https://www.mexc.com/api-docs/spot-v3/introduction): «IP-weighted endpoints share 300 weight per 10 seconds» — не
//! больше 300 / 10 / 3 = 10 снимков в секунду на IP. Процедура «How to Properly Maintain a Local Copy of the Order
//! Book», шаг 3: «Request https://api.mexc.com/api/v3/depth?symbol={symbol}&limit=5000».

use std::sync::LazyLock;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::{Mutex, mpsc};
use tokio::time::Instant;

use crate::core::types::PriceLevel;

use super::ctx::Ctx;

/// Наибольший темп: 300 веса за 10 с на IP («Rate Limits») / вес 3 (Order Book) = 10 в секунду.
pub const MAX_PER_SEC: u64 = 10;
/// Наибольший limit снимка — «max 5000» (Order Book).
pub const MAX_LIMIT: u64 = 5000;
/// Путь снимка.
const PATH: &str = "/api/v3/depth";
/// Когда можно слать следующий снимок (один адаптер MEXC на процесс — одна очередь на IP).
static NEXT: LazyLock<Mutex<Option<Instant>>> = LazyLock::new(|| Mutex::new(None));

/// Снимок пары в числах ядра.
pub struct Snap {
    pub id: i64,
    pub bids: Vec<PriceLevel>,
    pub asks: Vec<PriceLevel>,
}

/// Сообщение очереди соединению (символ MEXC).
pub enum Note {
    /// Очередь дошла, запрос уходит.
    Sent(String),
    /// Итог: снимок или причина сбоя.
    Done(String, Result<Snap, String>),
}

/// Что делает: проверяет настройки очереди. Возвращает: (пауза между запросами, limit). Ошибка — текст причины
/// (не задана или вне пределов документации).
pub fn config(ctx: &Ctx) -> Result<(Duration, u64), String> {
    let m = |e: crate::gateway::errors::GatewayError| e.message;
    let rate = ctx.int("venue.fast_book_snapshot_per_sec").map_err(m)?;
    if rate == 0 || rate > MAX_PER_SEC {
        return Err(format!(
            "venue.fast_book_snapshot_per_sec = {rate}: у MEXC от 1 до {MAX_PER_SEC} (300 веса / 10 с на IP, снимок — вес 3)"
        ));
    }
    let limit = ctx.int("venue.fast_book_snapshot_limit").map_err(m)?;
    if limit == 0 || limit > MAX_LIMIT {
        return Err(format!(
            "venue.fast_book_snapshot_limit = {limit}: у MEXC от 1 до {MAX_LIMIT}"
        ));
    }
    Ok((Duration::from_secs_f64(1.0 / rate as f64), limit))
}

/// Что делает: запускает задачу снимка пары: ждёт своей очереди, сообщает «запрос ушёл», запрашивает, разбирает и шлёт
/// итог в канал соединения. Принимает: окружение, символ MEXC, канал соединения. Ошибок нет (сбой — в итоге).
pub fn request(ctx: &Ctx, symbol: String, out: mpsc::Sender<Note>) {
    let ctx = ctx.clone();
    tokio::spawn(async move {
        let res = fetch(&ctx, &symbol, &out).await;
        let _ = out.send(Note::Done(symbol, res)).await;
    });
}

/// Очередь, запрос и разбор одного снимка. Ошибка — текст причины.
async fn fetch(ctx: &Ctx, symbol: &str, out: &mpsc::Sender<Note>) -> Result<Snap, String> {
    let (gap, limit) = config(ctx)?;
    {
        let mut next = NEXT.lock().await;
        if let Some(at) = *next {
            tokio::time::sleep_until(at).await;
        }
        *next = Some(Instant::now() + gap);
    }
    let _ = out.send(Note::Sent(symbol.to_string())).await;
    let q = vec![("symbol", symbol.to_string()), ("limit", limit.to_string())];
    let v = super::rest::public(ctx, PATH, q)
        .await
        .map_err(|e| e.message)?;
    parse(&v)
}

/// Что делает: разбирает ответ GET /api/v3/depth. Принимает: ответ. Возвращает: Snap. Ошибка — нет lastUpdateId,
/// уровни не разобраны (book_print.rs levels).
pub fn parse(v: &Value) -> Result<Snap, String> {
    let id = v
        .get("lastUpdateId")
        .and_then(Value::as_i64)
        .ok_or("в ответе нет lastUpdateId")?;
    let (bids, asks) = super::book_print::levels(v)?;
    Ok(Snap { id, bids, asks })
}
