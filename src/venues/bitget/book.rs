//! Файл: venues/bitget/book.rs
//! Что делает: последние стаканы Bitget в памяти и котировка на объём по уровням стакана
//! (сколько заплатим или получим за Q монет, с комиссией taker нашего аккаунта).
//! Решение 217: freeze — снимок стакана и плана комиссии (gateway/frozen.rs FrozenBook) для контракта quote_snapshot;
//! живая котировка — тот же снимок на один вызов (одна формула).
//! Кто вызывает: market.rs (обновляет стакан), mod.rs (quote для ядра; решение 201 — exec_needs_rest и priced для
//! снимка REST котировки исполнения, rest_book.rs).
//! От чего зависит: core/types, fees.rs, ctx.rs.
//! Настройки: feed.stale_after_ms (по площадке) — если соединение, по которому пришёл стакан, молчит дольше, данные
//! устарели; venue.book_stale_ms [bitget] (необязательна) — кадр стакана этой пары старше — котировки нет.
//!
//! Свежесть: живость соединения (любой кадр, включая "pong" на свой "ping") и, если задана venue.book_stale_ms, время
//! последнего кадра стакана самой пары (кадр с тем же seq тоже обновляет время — ws_conn.rs, решение 109). Шлёт ли
//! Bitget снимок books5/books15 без изменений рынка, на странице Depth Channel не сказано («Default data push frequency
//! for books, books5, books15 is 200ms») — проверить на живой записи; до того venue.book_stale_ms задавать с запасом.

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

/// Стакан пары, живость его соединения и время последнего кадра стакана пары (unix мс).
type Entry = (OrderBook, Alive, Alive);

/// Стаканы по тикеру. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Books {
    map: Arc<RwLock<HashMap<String, Entry>>>,
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
        m.insert(book.asset.ticker.clone(), (book, alive, seen));
    }

    /// Решение 201: стакан пары из снимка REST (котировка исполнения): заменяет стакан и время кадра пары, живость
    /// соединения — прежняя. Пары в памяти нет (подписки нет) — ничего. Возвращает: заменён ли. Ошибок нет.
    pub fn put_rest(&self, book: OrderBook) -> bool {
        let mut m = self.map.write().unwrap_or_else(|p| p.into_inner());
        match m.get(&book.asset.ticker).map(|(_, alive, _)| alive.clone()) {
            Some(alive) => {
                let seen: Alive = Arc::new(AtomicI64::new(now_ms()));
                m.insert(book.asset.ticker.clone(), (book, alive, seen));
                true
            }
            None => false,
        }
    }

    /// Кадр стакана пары пришёл, но снимок тот же (решение 109): только время кадра пары. Ошибок нет.
    pub fn touch(&self, ticker: &str) {
        let m = self.map.read().unwrap_or_else(|p| p.into_inner());
        if let Some((_, _, seen)) = m.get(ticker) {
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

    /// Котировка на объём.
    /// Принимает: окружение, кэш комиссий, символ Bitget пары, монету, сторону, количество монет.
    /// Возвращает: Quote (fillable_qty меньше qty, если глубины не хватает).
    /// Ошибки: StaleData — стакана нет, соединение молчит дольше feed.stale_after_ms или (если задана)
    /// кадра пары нет дольше venue.book_stale_ms; NotConfigured — feed.stale_after_ms не задана; комиссия.
    pub async fn quote(
        &self,
        ctx: &Ctx,
        fees: &Fees,
        symbol: &str,
        asset: &Asset,
        side: Side,
        qty: Decimal,
    ) -> GatewayResult<Quote> {
        Ok(self.freeze(ctx, fees, symbol, asset, side)?.price(qty))
    }

    /// Решение 217: снимок стакана потока монеты и плана комиссии на этот момент (копия стакана из памяти).
    /// Принимает: то же, что quote, без количества. Возвращает: FrozenBook. Ошибки: как у quote.
    pub fn freeze(
        &self,
        ctx: &Ctx,
        fees: &Fees,
        symbol: &str,
        asset: &Asset,
        side: Side,
    ) -> GatewayResult<FrozenBook> {
        let stale_ms = ctx.int("feed.stale_after_ms")? as i64;
        // Необязательна: см. шапку.
        let pair_ms = ctx.int("venue.book_stale_ms").ok().map(|v| v as i64);
        let book = self
            .map
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(&asset.ticker)
            .cloned();
        let (book, alive, seen) = book.ok_or_else(|| stale(ctx, &asset.ticker, "стакана нет"))?;
        let silent = now_ms() - alive.load(Ordering::Relaxed);
        if silent > stale_ms {
            return Err(stale(
                ctx,
                &asset.ticker,
                &format!("соединение молчит {silent} мс"),
            ));
        }
        let age = now_ms() - seen.load(Ordering::Relaxed);
        if let Some(pair_ms) = pair_ms
            && age > pair_ms
        {
            return Err(stale(
                ctx,
                &asset.ticker,
                &format!("кадра стакана пары нет {age} мс (venue.book_stale_ms = {pair_ms})"),
            ));
        }
        frozen(ctx, fees, symbol, asset, side, book)
    }

    /// Решение 201: нужен ли котировке ИСПОЛНЕНИЯ свежий снимок REST вместо стакана потока: стакана пары нет (поток ещё
    /// не пришёл, площадка без подписки) или кадр пары старше venue.book_stale_ms (неликвидная пара не получает
    /// кадров без изменений — RWA 06.11: 45–55 с при живом соединении). Принимает: окружение, тикер.
    /// Возвращает: да / нет. Ошибок нет.
    pub fn exec_needs_rest(&self, ctx: &Ctx, ticker: &str) -> bool {
        let m = self.map.read().unwrap_or_else(|p| p.into_inner());
        let Some((_, _, seen)) = m.get(ticker) else {
            return true;
        };
        let age = now_ms() - seen.load(Ordering::Relaxed);
        ctx.int("venue.book_stale_ms")
            .is_ok_and(|pair_ms| age > pair_ms as i64)
    }
}

/// Котировка на объём по данному стакану (поток или снимок REST): проход по уровням, комиссия taker.
/// Принимает: окружение, кэш комиссий, символ, монету, сторону, количество, стакан. Ошибки: комиссия.
pub fn priced(
    ctx: &Ctx,
    fees: &Fees,
    symbol: &str,
    asset: &Asset,
    side: Side,
    qty: Decimal,
    book: &OrderBook,
) -> GatewayResult<Quote> {
    Ok(frozen(ctx, fees, symbol, asset, side, book.clone())?.price(qty))
}

/// Решение 217: снимок стакана и плана комиссии (та же формула у живой котировки — priced).
/// Принимает: окружение, кэш комиссий, символ, монету, сторону, стакан (копия). Ошибки: комиссия.
pub fn frozen(
    ctx: &Ctx,
    fees: &Fees,
    symbol: &str,
    asset: &Asset,
    side: Side,
    book: OrderBook,
) -> GatewayResult<FrozenBook> {
    let rates = fees.get(ctx, symbol)?;
    Ok(FrozenBook {
        venue: ctx.venue(),
        asset: asset.clone(),
        side,
        book,
        // Комиссия taker аккаунта — в USDT-эквиваленте по сумме сделки (не входит в avg_price).
        fee: BookFee {
            rate: rates.taker,
            per_coin: false,
            asset: "USDT".into(),
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
        format!("Bitget {ticker}: {why} — котировки нет"),
    ))
}
