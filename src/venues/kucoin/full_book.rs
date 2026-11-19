//! Файл: venues/kucoin/full_book.rs
//! Что делает: решение 423 — снимки стакана REST Get Full OrderBook для локальных стаканов KuCoin (local_book.rs):
//! общая на все соединения очередь с темпом venue.full_book_per_sec — старт сотен пар и пересинхронизации после
//! разрывов идут по очереди, а не залпом: пул Spot общий с торговыми запросами (ордера, комиссии), 429000 закрыл бы
//! его и для ордеров. Снимок разбирается в числа ядра и отдаётся в канал своего соединения.
//! Кто вызывает: ws_fast.rs (ready, request), ws.rs (pace — проверка настройки), тесты (parse).
//! От чего зависит: rest.rs (signed — подпись и пауза пула после 429000), depth.rs (levels), reply.rs (Pool), errors.rs
//! (Op), ctx.rs.
//! Настройки: venue.full_book_per_sec [kucoin] (снимков в секунду; не задана — быстрого стакана нет), venue.api_key_version
//! и ключи kucoin.* (запрос подписанный).
//!
//! Официально (https://www.kucoin.com/docs-new/rest/spot-trading/market-data/get-full-orderbook): «GET
//! /api/v3/market/orderbook/level2», параметр symbol; api-channel Private, api-permission General, api-rate-limit-pool
//! Spot, api-rate-limit-weight 3; ответ data {time, sequence (строка), bids, asks — [цена, количество] строками}; «To
//! maintain up-to-date Order Book, please use Websocket incremental feed after retrieving the OrderBook». Квота пула
//! Spot — по UID за 30 с (https://www.kucoin.com/docs-new/rate-limit: VIP0 4000/30s): N снимков в секунду — 3 × N × 30
//! веса за окно.

use std::sync::LazyLock;
use std::time::Duration;

use reqwest::Method;
use serde_json::Value;
use tokio::sync::{Mutex, mpsc};
use tokio::time::Instant;

use crate::core::types::PriceLevel;

use super::ctx::Ctx;
use super::errors::Op;
use super::reply::Pool;
use super::{depth, rest};

/// Путь Get Full OrderBook.
pub const PATH: &str = "/api/v3/market/orderbook/level2";
/// Когда можно слать следующий снимок (один адаптер KuCoin на процесс — одна очередь на UID).
static NEXT: LazyLock<Mutex<Option<Instant>>> = LazyLock::new(|| Mutex::new(None));

/// Снимок пары в числах ядра.
#[derive(Debug, Clone, PartialEq)]
pub struct Snap {
    pub seq: i64,
    pub bids: Vec<PriceLevel>,
    pub asks: Vec<PriceLevel>,
}

/// Итог запроса снимка: символ и снимок или причина сбоя.
pub type Done = (String, Result<Snap, String>);

/// Сообщение очереди соединению (ревью 423, буфер).
#[derive(Debug)]
pub enum Event {
    /// Запрос снимка пары уходит из очереди — с этого момента кадры изменений пары копятся (local_book.rs start).
    Sent(String),
    /// Итог запроса.
    Done(Done),
}

/// Что делает: темп очереди — пауза между снимками (venue.full_book_per_sec). Ошибка — текст (не задана, 0).
pub fn pace(ctx: &Ctx) -> Result<Duration, String> {
    let rate = ctx.int("venue.full_book_per_sec").map_err(|e| e.message)?;
    if rate == 0 {
        return Err("venue.full_book_per_sec = 0: нужен хотя бы 1 снимок в секунду".into());
    }
    Ok(Duration::from_secs_f64(1.0 / rate as f64))
}

/// Что делает: можно ли сейчас просить снимок (темп задан, ключи разблокированы, версия ключа задана) — без запроса.
/// Ошибка — текст причины (запрос не ставится в очередь, вес пула не тратится).
pub fn ready(ctx: &Ctx) -> Result<(), String> {
    pace(ctx)?;
    ctx.keys().map_err(|e| e.message)?;
    ctx.text("venue.api_key_version").map_err(|e| e.message)?;
    Ok(())
}

/// Что делает: задача снимка пары: ждёт своей очереди, запрашивает, разбирает и шлёт итог в канал соединения.
/// Соединение закрылось, пока пара ждала очереди, — запрос не отправляется. Перед запросом — Event::Sent (ревью 423:
/// кадры пары копятся только с этого момента). Принимает: окружение, символ, канал.
/// Ошибок нет (сбой — в итоге).
pub fn request(ctx: &Ctx, symbol: String, out: mpsc::Sender<Event>) {
    let ctx = ctx.clone();
    tokio::spawn(async move {
        let res = fetch(&ctx, &symbol, &out).await;
        let _ = out.send(Event::Done((symbol, res))).await;
    });
}

/// Очередь, запрос и разбор одного снимка. Ошибка — текст причины.
async fn fetch(ctx: &Ctx, symbol: &str, out: &mpsc::Sender<Event>) -> Result<Snap, String> {
    let gap = pace(ctx)?;
    {
        let mut next = NEXT.lock().await;
        if let Some(at) = *next {
            tokio::time::sleep_until(at).await;
        }
        if out.is_closed() {
            return Err("соединение закрыто — снимок не нужен".into());
        }
        *next = Some(Instant::now() + gap);
    }
    // Ревью 423 (буфер): соединение начинает копить кадры пары до отправки запроса (сообщения канала — по порядку,
    // Sent раньше итога).
    let _ = out.send(Event::Sent(symbol.to_string())).await;
    let q = vec![("symbol", symbol.to_string())];
    let v = rest::signed(ctx, Method::GET, PATH, q, None, Op::Other, Pool::Spot)
        .await
        .map_err(|e| e.message)?;
    parse(&v).map_err(|why| format!("снимок {symbol} не разобран: {why}"))
}

/// Что делает: data ответа Get Full OrderBook → снимок (sequence — строка с целым; уровни — depth.rs levels, нулевые
/// без уровня). Принимает: data. Ошибка — текст причины.
pub fn parse(v: &Value) -> Result<Snap, String> {
    let seq = v
        .get("sequence")
        .and_then(Value::as_str)
        .and_then(|s| s.trim().parse().ok())
        .ok_or("нет sequence (строка с целым)")?;
    Ok(Snap {
        seq,
        bids: depth::levels(v, "bids")?,
        asks: depth::levels(v, "asks")?,
    })
}
