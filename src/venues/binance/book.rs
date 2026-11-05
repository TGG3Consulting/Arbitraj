//! Файл: venues/binance/book.rs
//! Что делает: последние стаканы Binance в памяти и котировка на объём по уровням стакана (сколько заплатим или
//! получим за Q монет) с комиссией тейкера нашего аккаунта (fee_rates.rs, вариант venue.fee_deduction).
//! Кто вызывает: ws_conn.rs (кладёт стакан), mod.rs (quote для ядра).
//! От чего зависит: core/types, fees.rs, fee_rates.rs, discount_balance.rs, ctx.rs.
//! Настройки: feed.stale_after_ms (соединение молчит дольше — данные устарели); venue.book_stale_ms [binance]
//! (обязательна) — кадра стакана пары нет дольше — котировки нет; venue.fee_deduction [binance] (обязательна);
//! venue.quote_asset.
//!
//! Частичный стакан <symbol>@depth<N>@100ms приходит снимками по таймеру (web-socket-streams «Partial Book Depth
//! Streams»: «Update Speed: 1000ms or 100ms») — одинаковый снимок ядру не отдаётся (решение 109), но время кадра пары
//! обновляется (touch). Две проверки свежести — как MEXC (решение 118, А-🟠8).
//! Комиссия (faqs/commission_faq): покупка платит комиссию полученной монетой — затрата в монете (ядро уменьшает
//! количество к выводу); продажа — котируемой монетой; со скидкой — в монете скидки (BNB), затрата в
//! эквиваленте котируемой монеты по сумме сделки, монета сделки не уменьшается.

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

/// Стакан пары, живость соединения и время последнего кадра стакана пары (unix мс).
type Entry = (OrderBook, Alive, Alive);

/// Стаканы по тикеру. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Books {
    map: Arc<RwLock<HashMap<String, Entry>>>,
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
        m.insert(book.asset.ticker.clone(), (book, alive, seen));
    }

    /// Кадр стакана пары пришёл, но снимок тот же (решение 109): только время кадра пары. Ошибок нет.
    pub fn touch(&self, ticker: &str) {
        let m = self.map.read().unwrap_or_else(|p| p.into_inner());
        if let Some((_, _, seen)) = m.get(ticker) {
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

    /// Котировка на объём. Принимает: окружение, ставки, монету, сторону, количество.
    /// Возвращает: Quote (fillable_qty меньше qty, если глубины не хватает).
    /// Ошибки: StaleData — стакана нет, соединение молчит или кадра пары нет дольше venue.book_stale_ms, ставки нет;
    /// NotConfigured — настройка не задана или вариант комиссии неизвестен.
    pub async fn quote(
        &self,
        ctx: &Ctx,
        fees: &Fees,
        asset: &Asset,
        side: Side,
        qty: Decimal,
    ) -> GatewayResult<Quote> {
        let stale_ms = ctx.int("feed.stale_after_ms")? as i64;
        let pair_ms = ctx.int("venue.book_stale_ms")? as i64;
        let quote_asset = ctx.text("venue.quote_asset")?;
        let entry = self
            .map
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(&asset.ticker)
            .cloned();
        let (book, alive, seen) = entry.ok_or_else(|| stale(ctx, &asset.ticker, "стакана нет"))?;
        let silent = now_ms() - alive.load(Ordering::Relaxed);
        if silent > stale_ms {
            let why = format!("соединение молчит {silent} мс");
            return Err(stale(ctx, &asset.ticker, &why));
        }
        let age = now_ms() - seen.load(Ordering::Relaxed);
        if age > pair_ms {
            let why = format!("кадра стакана пары нет {age} мс (venue.book_stale_ms = {pair_ms})");
            return Err(stale(ctx, &asset.ticker, &why));
        }
        let levels = match side {
            Side::Buy => &book.asks,
            Side::Sell => &book.bids,
        };
        let (filled, notional) = walk(levels, qty);
        let rates = fees.get(ctx, &ctx.symbol(&asset.ticker)?)?;
        let mode = ctx.text("venue.fee_deduction")?;
        // Решение 158: скидка — только если монеты скидки хватает (память discount_balance.rs).
        let (discount_ok, _) = fees.discount.ok(ctx);
        let (rate, in_discount, _why) = rates
            .plan(&mode, side, discount_ok)
            .map_err(|m| ctx.tag(GatewayError::not_set(None, "venue.fee_deduction", m)))?;
        let avg = if filled.is_zero() {
            Decimal::ZERO
        } else {
            notional / filled
        };
        let cost = if side == Side::Buy && !in_discount {
            Cost::new(CostKind::TradeFee, filled * rate, &asset.ticker, false)
        } else {
            Cost::new(CostKind::TradeFee, notional * rate, &quote_asset, false)
        };
        Ok(Quote {
            venue: ctx.venue(),
            asset: asset.clone(),
            side,
            qty,
            fillable_qty: filled,
            avg_price: avg,
            costs: vec![cost],
            route: None,
            signal_route: None,
            pick: None,
            at: book.received_at,
        })
    }
}

/// Проход по уровням от лучшего: сколько монет набрано и на какую сумму. Уровни отсортированы (depth.rs).
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

/// Ошибка «данные устарели». Ошибок нет.
fn stale(ctx: &Ctx, ticker: &str, why: &str) -> GatewayError {
    ctx.tag(GatewayError::new(
        ErrorCode::StaleData,
        None,
        format!("Binance {ticker}: {why} — котировки нет"),
    ))
}
