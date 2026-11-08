//! Файл: venues/xt/book.rs
//! Что делает: последние стаканы XT.com в памяти и котировка на объём по уровням стакана (сколько заплатим или
//! получим за Q монет) с комиссией тейкера пары (takerFeeRate справочника пар — ставки по ключу у XT нет).
//! Решение 217: freeze — снимок стакана и плана комиссии (gateway/frozen.rs FrozenBook) для контракта quote_snapshot;
//! живая котировка — тот же снимок на один вызов (одна формула).
//! Кто вызывает: ws_book.rs (кладёт стакан), mod.rs (quote для ядра, book_confirmed), book_version.rs (print).
//! От чего зависит: core/types, symbols.rs, ctx.rs.
//! Настройки: feed.stale_after_ms (соединение молчит дольше — данные устарели); venue.book_stale_ms [xt]
//! (обязательна) — кадра стакана пары нет дольше — котировки нет; venue.quote_asset.
//!
//! Снимки depth@{symbol},{levels} приходят раз в 1000 мс и при неизменном рынке — тем же updateId: такой снимок ядру
//! не отдаётся (решение 109), но время кадра пары обновляется (touch); локальный стакан depth_update — изменения.
//! Комиссия: ставка — takerFeeRate пары (https://doc.xt.com/#marketsymbol); эндпоинта ставки аккаунта (VIP-уровень)
//! в спот API v4 нет — вопрос Тиграну (отчёт решения 167). В какой монете берётся комиссия, документация не говорит;
//! как у других бирж — покупка с полученной монеты, продажа с полученных USDT (проверить feeCurrency живого ордера).
//! Скидка оплатой в XT (deductServices) в расчёте не учитывается: её ставку API не сообщает — расчёт по обычной
//! ставке (худший случай, урок 23), факт — из ордера (order_state.rs).
//! Решение 213 (Тигран 08.11, для всех бирж): тихая пара, которую сторож ядра подтвердил по REST (book_version.rs,
//! контракт book_confirmed), — свежая до срока подтверждения, даже если кадра нет дольше venue.book_stale_ms. Живость
//! соединения подтверждение не отменяет. Новый стакан (put) или снятие стакана — подтверждения нет (как у MEXC).

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, RwLock};

use chrono::Utc;
use rust_decimal::Decimal;

use crate::core::types::{Asset, OrderBook, Quote, Side};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::frozen::{BookFee, FrozenBook};

use super::ctx::Ctx;
use super::symbols::Symbols;

/// Время последнего сообщения соединения (unix мс) — общее для соединения и его стаканов.
pub type Alive = Arc<AtomicI64>;

/// Стакан пары, живость соединения, время последнего кадра стакана пары и срок подтверждения тихой пары
/// (решение 213; 0 — нет) (unix мс).
type Entry = (OrderBook, Alive, Alive, Alive);

/// Стаканы по тикеру. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Books {
    map: Arc<RwLock<HashMap<String, Entry>>>,
    /// Решение 202 (для всех бирж, 08.11): свежесть соединений и пар для сторожа ядра (gateway/feed_health.rs).
    pub feed: crate::gateway::feed_reg::FeedReg,
}

/// Сейчас, unix мс. Ошибок нет.
fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

impl Books {
    /// Сохраняет свежий стакан и связь с его соединением; время кадра пары — сейчас. Ошибок нет.
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

    /// Убирает стакан (данные недостоверны — оборвалось соединение, кадр не разобран). Ошибок нет.
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
        symbols: &Symbols,
        asset: &Asset,
        side: Side,
        qty: Decimal,
    ) -> GatewayResult<Quote> {
        Ok(self.freeze(ctx, symbols, asset, side)?.price(qty))
    }

    /// Решение 217: снимок стакана монеты и плана комиссии на этот момент (копия стакана из памяти).
    /// Принимает: то же, что quote, без количества. Возвращает: FrozenBook.
    /// Ошибки: StaleData — стакана нет, соединение молчит или кадра пары нет дольше venue.book_stale_ms и нет
    /// подтверждения тихой пары (решение 213), ставки нет; NotConfigured — настройка не задана.
    pub fn freeze(
        &self,
        ctx: &Ctx,
        symbols: &Symbols,
        asset: &Asset,
        side: Side,
    ) -> GatewayResult<FrozenBook> {
        let stale_ms = ctx.int("feed.stale_after_ms")? as i64;
        let pair_ms = ctx.int("venue.book_stale_ms")? as i64;
        let entry = self
            .map
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(&asset.ticker)
            .cloned();
        let (book, alive, seen, until) =
            entry.ok_or_else(|| stale(ctx, &asset.ticker, "стакана нет"))?;
        let silent = now_ms() - alive.load(Ordering::Relaxed);
        if silent > stale_ms {
            let why = format!("соединение молчит {silent} мс");
            return Err(stale(ctx, &asset.ticker, &why));
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
            let why =
                format!("кадра стакана пары нет {age} мс (venue.book_stale_ms = {pair_ms}){tail}");
            return Err(stale(ctx, &asset.ticker, &why));
        }
        frozen(ctx, symbols, asset, side, book)
    }
}

/// Решение 217: снимок стакана и плана комиссии (та же формула у стакана потока — freeze — и у снимка REST для
/// ордера — mod.rs quote_rest, решение 238). Принимает: окружение, данные комиссии, монету, сторону, стакан (во
/// владение). Ошибки: комиссия (ставки нет — StaleData), настройка не задана — NotConfigured.
pub fn frozen(
    ctx: &Ctx,
    symbols: &Symbols,
    asset: &Asset,
    side: Side,
    book: OrderBook,
) -> GatewayResult<FrozenBook> {
    let quote_asset = ctx.text("venue.quote_asset")?;
    let rate = symbols
        .taker_fee(&asset.ticker)
        .ok_or_else(|| stale(ctx, &asset.ticker, "XT не сообщил takerFeeRate пары"))?;
    // Покупка — комиссия в монете от количества, продажа — в котируемой монете от суммы. Решение 232: факт живого
    // бота 08.11 — покупка WIN: fee 1413.932 WIN = 0.002 × 706966 (в монете, 1/1); продаж XT в журнале нет.
    // Документацию по монете комиссии спота XT не нашёл: продажа в котируемой — не из документации (по получаемой
    // монете); скидка XT (deductServices) — факт ордера, в расчёте не учитывается.
    let fee = if side == Side::Buy {
        BookFee {
            rate,
            per_coin: true,
            asset: asset.ticker.clone(),
            paid_in: None,
        }
    } else {
        BookFee {
            rate,
            per_coin: false,
            asset: quote_asset,
            paid_in: None,
        }
    };
    Ok(FrozenBook {
        venue: ctx.venue(),
        asset: asset.clone(),
        side,
        book,
        // Комиссия тейкера — не входит в avg_price.
        fee,
        taken_at: chrono::Utc::now(),
    })
}

/// Проход по уровням от лучшего — общий для бирж (gateway/frozen.rs, решение 217).
pub use crate::gateway::frozen::walk;

/// Ошибка «данные устарели». Ошибок нет.
fn stale(ctx: &Ctx, ticker: &str, why: &str) -> GatewayError {
    ctx.tag(GatewayError::new(
        ErrorCode::StaleData,
        None,
        format!("XT {ticker}: {why} — котировки нет"),
    ))
}
