//! Файл: venues/gate/book.rs
//! Что делает: последние стаканы Gate в памяти и котировка на объём по уровням стакана
//! (сколько заплатим или получим за Q монет, с комиссией taker нашего аккаунта).
//! Кто вызывает: market.rs (обновляет стакан), mod.rs (quote для ядра).
//! От чего зависит: core/types, fees.rs, ctx.rs.
//! Настройки: feed.stale_after_ms (по площадке) — если соединение, по которому пришёл стакан, молчит дольше, данные
//! устарели; venue.book_stale_ms [gate] (обязательна) — кадр стакана этой пары старше — котировки нет;
//! venue.fee_deduction [gate] (обязательна) — вариант списания комиссии (fees.rs, Rates::plan).
//!
//! Комиссия в котировке (решение 135, Тигран 04.11): coin — покупка платит комиссию монетой сделки (затрата в монете:
//! ядро уменьшает на неё количество к выводу, core/costs.rs), продажа — USDT; gt — ставка со скидкой GT, затрата в
//! USDT-эквиваленте суммы, монета сделки не уменьшается (комиссию списывает GT). Факт — из ордера (order_state.rs).
//!
//! Две проверки свежести (как MEXC, решение 118, А-🟠8): живость соединения (любой кадр, включая spot.pong) и время
//! последнего кадра стакана самой пары — оно обновляется и тогда, когда снимок не изменился и ядру не отдаётся
//! (решение 109). Интервал канала spot.order_book (100ms / 1000ms, Spot WebSocket v4
//! https://www.gate.com/docs/developers/apiv4/ws/en/) — частота обновлений; что кадр приходит и без изменений книги,
//! в документации НЕ сказано, и живые данные это опровергают (решение 213: у неликвидных пар кадров нет часами при
//! живой подписке; в записи 04.11 кадры CAKE_USDT идут только с новым lastUpdateId, промежутки 290–380 мс при
//! 100ms). Без второй проверки пара со сбойной подпиской считалась бы свежей бесконечно.
//! Решение 213: тихая пара, у которой сторож ядра подтвердил по REST «книга не менялась с последнего кадра»
//! (контракт book_confirmed), — свежая до срока подтверждения, даже если кадра нет дольше venue.book_stale_ms.
//! Живость соединения подтверждение не отменяет. Новый стакан из кадра (put) или снятие стакана — подтверждения нет.

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

/// Стакан пары, живость его соединения, время последнего кадра стакана пары и срок подтверждения тихой пары
/// (решение 213; 0 — нет) (unix мс).
type Entry = (OrderBook, Alive, Alive, Alive);

/// Стаканы по тикеру. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Books {
    map: Arc<RwLock<HashMap<String, Entry>>>,
    /// Решение 202: свежесть соединений и пар для сторожа ядра (gateway/feed_health.rs).
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

    /// Решение 213: сторож ядра подтвердил по REST, что книга пары на бирже не менялась с последнего кадра, —
    /// стакан в памяти свежий до until_ms (unix мс). Стакана нет — ничего. Ошибок нет.
    pub fn confirm(&self, ticker: &str, until_ms: i64) {
        let m = self.map.read().unwrap_or_else(|p| p.into_inner());
        if let Some((_, _, _, until)) = m.get(ticker) {
            until.fetch_max(until_ms, Ordering::Relaxed);
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
    /// Принимает: окружение, кэш комиссий, монету, сторону, количество монет.
    /// Возвращает: Quote (fillable_qty меньше qty, если глубины не хватает).
    /// Ошибки: StaleData — стакана нет, соединение молчит дольше feed.stale_after_ms или кадра пары нет дольше
    /// venue.book_stale_ms и нет подтверждения тихой пары (решение 213); NotConfigured — настройка не задана; комиссия.
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
        let levels = match side {
            Side::Buy => &book.asks,
            Side::Sell => &book.bids,
        };
        let (filled, notional) = walk(levels, qty);
        let rates = fees.get(ctx, &ctx.symbol(&asset.ticker)?)?;
        let mode = ctx.text("venue.fee_deduction")?;
        // Решение 158: скидка GT — только если GT хватает (память gt_balance.rs), иначе обычная ставка.
        let (gt_ok, _) = fees.gt.ok(ctx);
        let (rate, in_gt, _why) = rates
            .plan(&mode, gt_ok)
            .map_err(|m| ctx.tag(GatewayError::not_set(None, "venue.fee_deduction", m)))?;
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
            // Комиссия taker аккаунта (не входит в avg_price): покупка без GT — в монете, иначе — USDT по сумме.
            costs: vec![if side == Side::Buy && !in_gt {
                Cost::new(CostKind::TradeFee, filled * rate, &asset.ticker, false)
            } else {
                Cost::new(CostKind::TradeFee, notional * rate, "USDT", false)
            }],
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
        format!("Gate {ticker}: {why} — котировки нет"),
    ))
}
