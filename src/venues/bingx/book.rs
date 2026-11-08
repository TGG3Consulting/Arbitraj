//! Файл: venues/bingx/book.rs
//! Что делает: последние стаканы BingX в памяти и котировка на объём по уровням стакана
//! (сколько заплатим или получим за Q монет, с комиссией taker нашего аккаунта).
//! Решение 217: freeze — снимок стакана и плана комиссии (gateway/frozen.rs FrozenBook) для контракта quote_snapshot;
//! живая котировка — тот же снимок на один вызов (одна формула).
//! Кто вызывает: ws_conn.rs (обновляет стакан), mod.rs (quote для ядра; book_confirmed), book_print.rs (print).
//! От чего зависит: core/types, fees.rs, ctx.rs.
//! Настройки: feed.stale_after_ms (по площадке) — если соединение, по которому пришёл стакан,
//! молчит дольше, данные устарели и котировки нет; venue.book_stale_ms [bingx] (обязательна — решение 238) — кадра
//! стакана этой пары нет дольше и нет подтверждения тихой пары — котировки нет.
//!
//! Важно (реальная запись 2026-10-23): BingX присылает стакан @depth<N> только при изменении,
//! а не каждые 300 мс, как в описании. Поэтому «свежесть» — это живость соединения
//! (любое сообщение, включая ping раз в ~30 с), а не возраст самого стакана.
//! Решения 202/213 (для всех бирж, Тигран 08.11): время последнего кадра пары и срок подтверждения тихой пары. Если
//! venue.book_stale_ms задана (сторожу пар она нужна), пара без кадра дольше неё свежа, только пока действует
//! подтверждение сторожа ядра «первые N уровней стакана на бирже совпадают со стаканом в памяти» (book_print.rs,
//! контракт book_confirmed). Решение 238 (Тигран 09.11: «ни одна биржа не идёт в сделку по устаревшему стакану»):
//! настройка обязательна, как у MEXC, Binance, Bybit, Gate, XT — не задана: котировки нет (раньше — только живость
//! соединения). Ордер при устаревшем стакане — снимок REST (core/exec_book.rs, mod.rs quote_rest). Новый стакан (put)
//! или снятие стакана — подтверждения нет (как у MEXC, Bitget, Gate).

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

/// Стаканы по тикеру вместе с живостью их соединения. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Books {
    map: Arc<RwLock<HashMap<String, Entry>>>,
    /// Решение 202 (для всех бирж, 08.11): свежесть соединений и пар для сторожа ядра (gateway/feed_health.rs).
    pub feed: crate::gateway::feed_reg::FeedReg,
}

impl Books {
    /// Сохраняет свежий стакан и связь с его соединением; время кадра пары — сейчас.
    pub fn put(&self, book: OrderBook, alive: Alive) {
        let mut m = self.map.write().unwrap_or_else(|p| p.into_inner());
        let seen: Alive = Arc::new(AtomicI64::new(Utc::now().timestamp_millis()));
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

    /// Убирает стакан (данные недостоверны — например, оборвалось соединение).
    pub fn drop_ticker(&self, ticker: &str) {
        self.map
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .remove(ticker);
    }

    /// Котировка на объём (снимок на один вызов — та же формула, что у снимка решения 217).
    /// Принимает: окружение, кэш комиссий, монету, сторону, количество монет.
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
    /// Принимает: окружение, кэш комиссий, монету, сторону. Возвращает: FrozenBook.
    /// Ошибки: StaleData — стакана нет, соединение молчит дольше feed.stale_after_ms или кадра пары нет дольше
    /// venue.book_stale_ms и нет подтверждения тихой пары (решение 213); NotConfigured — настройка; комиссия.
    pub fn freeze(
        &self,
        ctx: &Ctx,
        fees: &Fees,
        asset: &Asset,
        side: Side,
    ) -> GatewayResult<FrozenBook> {
        let stale_ms = ctx.int("feed.stale_after_ms")? as i64;
        // Решение 238: обязательна у всех бирж (не задана — котировки нет).
        let pair_ms = ctx.int("venue.book_stale_ms")? as i64;
        let book = self
            .map
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(&asset.ticker)
            .cloned();
        let (book, alive, seen, until) =
            book.ok_or_else(|| stale(ctx, &asset.ticker, "стакана нет"))?;
        let now = Utc::now().timestamp_millis();
        let silent = now - alive.load(Ordering::Relaxed);
        if silent > stale_ms {
            return Err(stale(
                ctx,
                &asset.ticker,
                &format!("соединение молчит {silent} мс"),
            ));
        }
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

/// Решение 217: снимок стакана и плана комиссии (та же формула у стакана потока — freeze — и у снимка REST для
/// ордера — mod.rs quote_rest, решение 238). Принимает: окружение, данные комиссии, монету, сторону, стакан (во
/// владение). Ошибки: комиссия (ставки нет — StaleData), настройка не задана — NotConfigured.
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
        // Комиссия taker аккаунта (не входит в avg_price). Решение 219: «Fees on BingX Spot are settled in the received
        // asset» (BingX Learn «What Are BingX Spot Trading Fees for Makers and Takers»; факт 08.11: покупка — 392/392 в
        // монете): покупка — в монете от количества, продажа — в USDT от суммы.
        fee: BookFee {
            rate: rates.taker,
            per_coin: side == Side::Buy,
            asset: if side == Side::Buy {
                asset.ticker.clone()
            } else {
                "USDT".into()
            },
            paid_in: None,
        },
        taken_at: Utc::now(),
    })
}

/// Проход по уровням от лучшего — общий для бирж (gateway/frozen.rs, решение 217).
pub use crate::gateway::frozen::walk;

/// Ошибка «данные устарели».
fn stale(ctx: &Ctx, ticker: &str, why: &str) -> GatewayError {
    ctx.tag(GatewayError::new(
        ErrorCode::StaleData,
        None,
        format!("BingX {ticker}: {why} — котировки нет"),
    ))
}
