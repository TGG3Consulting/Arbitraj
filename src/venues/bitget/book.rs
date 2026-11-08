//! Файл: venues/bitget/book.rs
//! Что делает: последние стаканы Bitget в памяти и котировка на объём по уровням стакана
//! (сколько заплатим или получим за Q монет, с комиссией taker нашего аккаунта).
//! Решение 217: freeze — снимок стакана и плана комиссии (gateway/frozen.rs FrozenBook) для контракта quote_snapshot;
//! живая котировка — тот же снимок на один вызов (одна формула). freeze_decided (quote_snapshot) — тот же стакан, по
//! которому котировали, включая снимок REST котировки исполнения у пары без стакана потока (решение 201) — book_decided.rs.
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
//! Решение 213 (по содержимому, Тигран 08.11): тихая пара, у которой сторож ядра подтвердил по REST «первые N уровней
//! стакана на бирже совпадают со стаканом в памяти» (book_print.rs, контракт book_confirmed), — свежая до срока
//! подтверждения, даже если кадра нет дольше venue.book_stale_ms. Живость соединения подтверждение не отменяет. Новый
//! стакан (put, put_rest) или снятие стакана — подтверждения нет (как у Gate). Котировка исполнения (решение 201,
//! exec_needs_rest) по-прежнему берёт снимок REST, если кадр пары старше venue.book_stale_ms.

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
    /// Решение 217 (исправление по проверке): снимок REST котировки исполнения пары без стакана потока (решение 201,
    /// put_rest не заменил стакан) и когда получен (unix мс) — для снимка рынка на момент решения (freeze_decided).
    pub(super) rest: Arc<RwLock<HashMap<String, (OrderBook, i64)>>>,
    /// Решение 202 (для всех бирж, 08.11): свежесть соединений и пар потока для сторожа ядра (gateway/feed_health.rs).
    /// Снимок REST котировки исполнения (put_rest) сюда не попадает — только кадры WebSocket (ws_conn.rs).
    pub feed: crate::gateway::feed_reg::FeedReg,
}

/// Сейчас, unix мс.
pub(super) fn now_ms() -> i64 {
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
    /// котировка (стакан потока; после котировки исполнения — снимок REST, put_rest). Принимает: тикер, n.
    /// Возвращает: отпечаток; None — стакана нет. Ошибок нет.
    pub fn print(&self, ticker: &str, n: usize) -> Option<crate::gateway::feed_health::BookPrint> {
        let m = self.map.read().unwrap_or_else(|p| p.into_inner());
        m.get(ticker)
            .map(|(b, ..)| crate::gateway::feed_health::BookPrint::top(&b.bids, &b.asks, n))
    }

    /// Решение 201: стакан пары из снимка REST (котировка исполнения): заменяет стакан и время кадра пары, живость
    /// соединения — прежняя. Пары в памяти нет (подписки нет) — стакан потока не появляется, снимок запоминается
    /// отдельно для снимка рынка на момент решения (решение 217, freeze_decided). Возвращает: заменён ли стакан
    /// потока. Ошибок нет.
    pub fn put_rest(&self, book: OrderBook) -> bool {
        let mut m = self.map.write().unwrap_or_else(|p| p.into_inner());
        match m
            .get(&book.asset.ticker)
            .map(|(_, alive, ..)| alive.clone())
        {
            Some(alive) => {
                let seen: Alive = Arc::new(AtomicI64::new(now_ms()));
                let until: Alive = Arc::new(AtomicI64::new(0));
                m.insert(book.asset.ticker.clone(), (book, alive, seen, until));
                true
            }
            None => {
                drop(m);
                let mut r = self.rest.write().unwrap_or_else(|p| p.into_inner());
                r.insert(book.asset.ticker.clone(), (book, now_ms()));
                false
            }
        }
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

    /// Котировка на объём.
    /// Принимает: окружение, кэш комиссий, символ Bitget пары, монету, сторону, количество монет.
    /// Возвращает: Quote (fillable_qty меньше qty, если глубины не хватает).
    /// Ошибки: StaleData — стакана нет, соединение молчит дольше feed.stale_after_ms или (если задана)
    /// кадра пары нет дольше venue.book_stale_ms и нет подтверждения тихой пары (решение 213); NotConfigured — feed.stale_after_ms не задана; комиссия.
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
        if let Some(pair_ms) = pair_ms
            && age > pair_ms
            && now > until
        {
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
        frozen(ctx, fees, symbol, asset, side, book)
    }

    /// Решение 201: нужен ли котировке ИСПОЛНЕНИЯ свежий снимок REST вместо стакана потока: стакана пары нет (поток ещё
    /// не пришёл, площадка без подписки) или кадр пары старше venue.book_stale_ms (неликвидная пара не получает
    /// кадров без изменений — RWA 06.11: 45–55 с при живом соединении). Принимает: окружение, тикер.
    /// Возвращает: да / нет. Ошибок нет.
    pub fn exec_needs_rest(&self, ctx: &Ctx, ticker: &str) -> bool {
        let m = self.map.read().unwrap_or_else(|p| p.into_inner());
        let Some((_, _, seen, _)) = m.get(ticker) else {
            return true;
        };
        let age = now_ms() - seen.load(Ordering::Relaxed);
        ctx.int("venue.book_stale_ms")
            .is_ok_and(|pair_ms| age > pair_ms as i64)
    }
}

/// Котировка на объём по данному стакану (поток или снимок REST): проход по уровням, комиссия taker.
/// Принимает: окружение, кэш комиссий, символ, монету, сторону, количество, стакан (во владение — без копии).
/// Ошибки: комиссия.
pub fn priced(
    ctx: &Ctx,
    fees: &Fees,
    symbol: &str,
    asset: &Asset,
    side: Side,
    qty: Decimal,
    book: OrderBook,
) -> GatewayResult<Quote> {
    Ok(frozen(ctx, fees, symbol, asset, side, book)?.price(qty))
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
        // Комиссия taker аккаунта (не входит в avg_price). Решение 219: Bitget берёт комиссию из получаемой монеты
        // (Get Order Info, feeDetail.newFees: t — «total trading fees, unit: currency obtained from the transaction»; факт
        // 08.11: покупка — в монете): покупка — в монете от количества, продажа — в USDT от суммы. Скидка BGB — часть или
        // вся комиссия в BGB, размер скидки в документации нет (fees.rs): расчёт по обычной ставке в получаемой монете —
        // худший случай для количества монет; не хватит BGB — Bitget сам берёт остаток из получаемой монеты (r).
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
