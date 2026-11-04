//! Файл: venues/okx/book.rs
//! Что делает: последние стаканы OKX в памяти и котировка на объём по уровням стакана
//! (сколько заплатим или получим за Q монет, с комиссией taker нашего аккаунта).
//! Кто вызывает: ws_conn.rs (обновляет стакан), mod.rs (quote для ядра).
//! От чего зависит: core/types, fees.rs, ctx.rs.
//! Настройки: feed.stale_after_ms (по площадке) — если соединение, по которому пришёл стакан, молчит дольше, данные
//! устарели; venue.book_stale_ms [okx] (необязательна) — кадр стакана этой пары старше — котировки нет.
//!
//! Свежесть: живость соединения (любой кадр, включая pong на свой «ping») и, если задана venue.book_stale_ms, время
//! последнего кадра стакана самой пары. У OKX вторая проверка не обязательна: books5 — «Snapshot data will be pushed
//! every 100 ms when there are changes» (без изменений кадров нет), books — пустое изменение «If there are no updates to
//! the depth for an extended period(Around 60 seconds)» (https://www.okx.com/docs-v5/en/, «Order book channel») —
//! тихая пара без изменений иначе считалась бы устаревшей.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, RwLock};

use chrono::Utc;
use rust_decimal::Decimal;

use crate::core::types::{Asset, Cost, CostKind, OrderBook, PriceLevel, Quote, Side};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

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
    /// Принимает: окружение, кэш комиссий, символ OKX пары, монету, сторону, количество монет.
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
        let stale_ms = ctx.int("feed.stale_after_ms")? as i64;
        // Необязательна у OKX: снимок без изменений рынка не присылается (см. шапку).
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
        let levels = match side {
            Side::Buy => &book.asks,
            Side::Sell => &book.bids,
        };
        let (filled, notional) = walk(levels, qty);
        let rates = fees.get(ctx, symbol)?;
        let avg = if filled.is_zero() {
            Decimal::ZERO
        } else {
            notional / filled
        };
        Ok(Quote {
            venue: ctx.venue(),
            asset: asset.clone(),
            side,
            qty,
            fillable_qty: filled,
            avg_price: avg,
            // Комиссия taker аккаунта — в USDT-эквиваленте по сумме сделки (не входит в avg_price).
            costs: vec![Cost::new(
                CostKind::TradeFee,
                notional * rates.taker,
                "USDT",
                false,
            )],
            route: None,
            signal_route: None,
            pick: None,
            at: book.received_at,
        })
    }
}

/// Проход по уровням от лучшего: сколько монет набрано и на какую сумму.
/// Уровни уже отсортированы от лучшей цены к худшей (market.rs).
pub fn walk(levels: &[PriceLevel], qty: Decimal) -> (Decimal, Decimal) {
    let (mut left, mut filled, mut notional) = (qty, Decimal::ZERO, Decimal::ZERO);
    for l in levels {
        if left <= Decimal::ZERO {
            break;
        }
        let take = left.min(l.qty);
        filled += take;
        notional += take * l.price;
        left -= take;
    }
    (filled, notional)
}

/// Ошибка «данные устарели».
fn stale(ctx: &Ctx, ticker: &str, why: &str) -> GatewayError {
    ctx.tag(GatewayError::new(
        ErrorCode::StaleData,
        None,
        format!("OKX {ticker}: {why} — котировки нет"),
    ))
}
