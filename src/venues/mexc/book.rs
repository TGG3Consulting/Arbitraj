//! Файл: venues/mexc/book.rs
//! Что делает: последние стаканы MEXC в памяти и котировка на объём по уровням стакана
//! (сколько заплатим или получим за Q монет, с комиссией taker нашего аккаунта).
//! Решение 217: freeze — снимок стакана и плана комиссии (gateway/frozen.rs FrozenBook) для контракта quote_snapshot;
//! живая котировка — тот же снимок на один вызов (одна формула).
//! Кто вызывает: market.rs (обновляет стакан), mod.rs (quote для ядра).
//! От чего зависит: core/types, fees.rs, ctx.rs.
//! Настройки: feed.stale_after_ms (по площадке) — если соединение, по которому пришёл стакан, молчит дольше, данные
//! устарели; venue.book_stale_ms [mexc] (обязательна) — кадр стакана этой пары старше — котировки нет.
//!
//! Две проверки свежести (решение 118, А-🟠8): живость соединения (любой кадр, включая PONG) и время последнего кадра
//! стакана самой пары — оно обновляется и тогда, когда снимок не изменился и ядру не отдаётся (решение 109). Частота
//! частичного стакана spot@public.limit.depth.v3.api.pb в документации MEXC не указана: пара, по которой MEXC перестал
//! присылать кадры (пауза торгов, сбой подписки), без второй проверки считалась бы свежей бесконечно.
//! Решение 213 (по содержимому, Тигран 08.11): тихая пара, у которой сторож ядра подтвердил по REST «первые N уровней
//! стакана на бирже совпадают со стаканом в памяти» (book_print.rs, контракт book_confirmed), — свежая до срока
//! подтверждения, даже если кадра нет дольше venue.book_stale_ms. Живость соединения подтверждение не отменяет. Новый
//! стакан из кадра (put) или снятие стакана — подтверждения нет (как у Gate).

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, RwLock};

use chrono::Utc;
use rust_decimal::Decimal;

use crate::core::types::{Asset, OrderBook, Quote, Side};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::frozen::{BookFee, FrozenBook};

use super::ctx::Ctx;
use super::fees::Fees;

/// Время последнего сообщения соединения (unix мс) — общее для соединения и его стаканов.
pub type Alive = Arc<AtomicI64>;

/// Стакан пары, живость его соединения, время последнего кадра стакана пары и срок подтверждения тихой пары
/// (решение 213; 0 — нет) (unix мс).
type Entry = (OrderBook, Alive, Alive, Alive);

/// Стаканы по тикеру. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Books {
    map: Arc<RwLock<HashMap<String, Entry>>>,
    /// Решение 202 (для всех бирж, 08.11): свежесть соединений и пар для сторожа ядра (gateway/feed_health.rs).
    pub feed: crate::gateway::feed_reg::FeedReg,
}

/// Сейчас, unix мс.
fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

impl Books {
    /// Сохраняет свежий стакан и связь с его соединением; время кадра пары — сейчас.
    pub fn put(&self, book: OrderBook, alive: Alive) {
        let mut m = self.map.write().unwrap_or_else(|p| p.into_inner());
        let seen: Alive = Arc::new(AtomicI64::new(now_ms()));
        let until: Alive = Arc::new(AtomicI64::new(0));
        m.insert(book.asset.ticker.clone(), (book, alive, seen, until));
    }

    /// Решение 213: сторож ядра подтвердил по REST, что книга пары на бирже совпадает со стаканом в памяти, — стакан
    /// свежий до until_ms (unix мс). Стакана нет — ничего. Ошибок нет.
    pub fn confirm(&self, ticker: &str, until_ms: i64) {
        let m = self.map.read().unwrap_or_else(|p| p.into_inner());
        if let Some((_, _, _, until)) = m.get(ticker) {
            until.fetch_max(until_ms, Ordering::Relaxed);
        }
    }

    /// Решение 213 (по содержимому): первые n уровней обеих сторон стакана пары в памяти — того, по которому считается
    /// котировка. Принимает: тикер, n. Возвращает: отпечаток; None — стакана нет. Ошибок нет.
    pub fn print(&self, ticker: &str, n: usize) -> Option<crate::gateway::feed_health::BookPrint> {
        let m = self.map.read().unwrap_or_else(|p| p.into_inner());
        m.get(ticker)
            .map(|(b, ..)| crate::gateway::feed_health::BookPrint::top(&b.bids, &b.asks, n))
    }

    /// Кадр стакана пары пришёл, но снимок тот же (решение 109): только время кадра пары. Ошибок нет.
    pub fn touch(&self, ticker: &str) {
        let m = self.map.read().unwrap_or_else(|p| p.into_inner());
        if let Some((_, _, seen, _)) = m.get(ticker) {
            seen.store(now_ms(), Ordering::Relaxed);
        }
    }

    /// Убирает стакан (данные недостоверны — например, оборвалось соединение).
    pub fn drop_ticker(&self, ticker: &str) {
        self.map
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .remove(ticker);
    }

    /// Котировка на объём (снимок на один вызов — та же формула, что у снимка решения 217).
    /// Принимает: окружение, данные комиссии, монету, сторону, количество монет.
    /// Возвращает: Quote (fillable_qty меньше qty, если глубины не хватает). Ошибки: как у freeze.
    pub async fn quote(
        &self,
        ctx: &Ctx,
        fees: &Fees,
        asset: &Asset,
        side: Side,
        qty: Decimal,
    ) -> GatewayResult<Quote> {
        Ok(self.freeze(ctx, fees, asset, side)?.price(qty))
    }

    /// Решение 217: снимок стакана монеты и плана комиссии на этот момент (копия стакана из памяти).
    /// Принимает: то же, что quote, без количества. Возвращает: FrozenBook.
    /// Ошибки: StaleData — стакана нет, соединение молчит дольше feed.stale_after_ms или кадра пары нет дольше
    /// venue.book_stale_ms и нет подтверждения тихой пары (решение 213); NotConfigured — настройка не задана; комиссия.
    pub fn freeze(
        &self,
        ctx: &Ctx,
        fees: &Fees,
        asset: &Asset,
        side: Side,
    ) -> GatewayResult<FrozenBook> {
        let stale_ms = ctx.int("feed.stale_after_ms")? as i64;
        let pair_ms = ctx.int("venue.book_stale_ms")? as i64;
        let book = self
            .map
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(&asset.ticker)
            .cloned();
        let (book, alive, seen, until) =
            book.ok_or_else(|| stale(ctx, &asset.ticker, "стакана нет"))?;
        let silent = now_ms() - alive.load(Ordering::Relaxed);
        if silent > stale_ms {
            return Err(stale(
                ctx,
                &asset.ticker,
                &format!("соединение молчит {silent} мс"),
            ));
        }
        let now = now_ms();
        let age = now - seen.load(Ordering::Relaxed);
        let until = until.load(Ordering::Relaxed);
        if age > pair_ms && now > until {
            let tail = if until > 0 {
                format!(", подтверждение биржи истекло {} мс назад", now - until)
            } else {
                String::new()
            };
            return Err(stale(
                ctx,
                &asset.ticker,
                &format!("кадра стакана пары нет {age} мс (venue.book_stale_ms = {pair_ms}){tail}"),
            ));
        }
        frozen(ctx, fees, asset, side, book)
    }
}

/// Решение 217: снимок стакана и плана комиссии (та же формула у стакана потока — freeze — и у снимка REST для ордера —
/// mod.rs quote_rest, решение 238). Принимает: окружение, данные комиссии, монету, сторону, стакан (во владение).
/// Ошибки: StaleData — ставка комиссии ещё не получена; NotConfigured — venue.quote_asset.
pub fn frozen(
    ctx: &Ctx,
    fees: &Fees,
    asset: &Asset,
    side: Side,
    book: OrderBook,
) -> GatewayResult<FrozenBook> {
    let rates = fees.get(ctx, &ctx.symbol(&asset.ticker)?)?;
    Ok(FrozenBook {
        venue: ctx.venue(),
        asset: asset.clone(),
        side,
        book,
        // Комиссия taker аккаунта — в USDT по сумме сделки (не входит в avg_price). Решение 219: MEXC берёт комиссию
        // в валюте рынка (котируемой) — MEXC Learn «Calculation of Fees in Spot Trading»; факт 08.11: покупка 214/214
        // и продажа 127/127 — USDT; в списание покупки входит (costs_types.rs spend_fee). Не из документации: при
        // включённом MX Deduction («the system prioritizes using MX tokens to pay fees») комиссия уйдёт в MX —
        // состояние MX Deduction не читается; расчёт в USDT — худший случай для списания USDT.
        fee: BookFee {
            rate: rates.taker,
            per_coin: false,
            asset: "USDT".into(),
            paid_in: None,
        },
        taken_at: chrono::Utc::now(),
    })
}

/// Проход по уровням от лучшего — общий для бирж (gateway/frozen.rs, решение 217).
pub use crate::gateway::frozen::walk;

/// Ошибка «данные устарели».
fn stale(ctx: &Ctx, ticker: &str, why: &str) -> GatewayError {
    ctx.tag(GatewayError::new(
        ErrorCode::StaleData,
        None,
        format!("MEXC {ticker}: {why} — котировки нет"),
    ))
}
