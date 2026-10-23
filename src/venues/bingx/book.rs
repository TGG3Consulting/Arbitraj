//! Файл: venues/bingx/book.rs
//! Что делает: последние стаканы BingX в памяти и котировка на объём по уровням стакана
//! (сколько заплатим или получим за Q монет, с комиссией taker нашего аккаунта).
//! Кто вызывает: market.rs (обновляет стакан), mod.rs (quote для ядра).
//! От чего зависит: core/types, fees.rs, ctx.rs.
//! Настройки: feed.stale_after_ms (по площадке) — если соединение, по которому пришёл стакан,
//! молчит дольше, данные устарели и котировки нет.
//!
//! Важно (реальная запись 2026-10-23): BingX присылает стакан @depth<N> только при изменении,
//! а не каждые 300 мс, как в описании. Поэтому «свежесть» — это живость соединения
//! (любое сообщение, включая ping раз в ~30 с), а не возраст самого стакана.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, RwLock};

use chrono::Utc;
use rust_decimal::Decimal;

use crate::core::types::{Asset, OrderBook, PriceLevel, Quote, Side};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::fees::Fees;

/// Время последнего сообщения соединения (unix мс) — общее для соединения и его стаканов.
pub type Alive = Arc<AtomicI64>;

/// Стаканы по тикеру вместе с живостью их соединения. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Books {
    map: Arc<RwLock<HashMap<String, (OrderBook, Alive)>>>,
}

impl Books {
    /// Сохраняет свежий стакан и связь с его соединением.
    pub fn put(&self, book: OrderBook, alive: Alive) {
        let mut m = self.map.write().unwrap_or_else(|p| p.into_inner());
        m.insert(book.asset.ticker.clone(), (book, alive));
    }

    /// Убирает стакан (данные недостоверны — например, оборвалось соединение).
    pub fn drop_ticker(&self, ticker: &str) {
        self.map
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .remove(ticker);
    }

    /// Котировка на объём.
    /// Принимает: окружение, кэш комиссий, монету, сторону, количество монет.
    /// Возвращает: Quote (fillable_qty меньше qty, если глубины не хватает).
    /// Ошибки: StaleData — стакана нет или соединение молчит дольше feed.stale_after_ms; комиссия.
    pub async fn quote(
        &self,
        ctx: &Ctx,
        fees: &Fees,
        asset: &Asset,
        side: Side,
        qty: Decimal,
    ) -> GatewayResult<Quote> {
        let stale_ms = ctx.int("feed.stale_after_ms")? as i64;
        let book = self
            .map
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(&asset.ticker)
            .cloned();
        let (book, alive) = book.ok_or_else(|| stale(ctx, &asset.ticker, "стакана нет"))?;
        let silent = Utc::now().timestamp_millis() - alive.load(Ordering::Relaxed);
        if silent > stale_ms {
            return Err(stale(
                ctx,
                &asset.ticker,
                &format!("соединение молчит {silent} мс"),
            ));
        }
        let levels = match side {
            Side::Buy => &book.asks,
            Side::Sell => &book.bids,
        };
        let (filled, notional) = walk(levels, qty);
        let rates = fees.get(ctx, &ctx.symbol(&asset.ticker)?)?;
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
            fee_usdt: notional * rates.taker,
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
        format!("BingX {ticker}: {why} — котировки нет"),
    ))
}
