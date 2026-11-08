//! Файл: venues/bingx/book.rs
//! Что делает: последние стаканы BingX в памяти и котировка на объём по уровням стакана
//! (сколько заплатим или получим за Q монет, с комиссией taker нашего аккаунта).
//! Решение 217: freeze — снимок стакана и плана комиссии (gateway/frozen.rs FrozenBook) для контракта quote_snapshot;
//! живая котировка — тот же снимок на один вызов (одна формула).
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

use crate::core::types::{Asset, OrderBook, Quote, Side};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::frozen::{BookFee, FrozenBook};

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
    /// Ошибки: StaleData — стакана нет или соединение молчит дольше feed.stale_after_ms; комиссия.
    pub fn freeze(
        &self,
        ctx: &Ctx,
        fees: &Fees,
        asset: &Asset,
        side: Side,
    ) -> GatewayResult<FrozenBook> {
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
