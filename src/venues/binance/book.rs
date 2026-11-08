//! Файл: venues/binance/book.rs
//! Что делает: последние стаканы Binance в памяти и котировка на объём по уровням стакана (сколько заплатим или
//! получим за Q монет) с комиссией тейкера нашего аккаунта (fee_rates.rs, вариант venue.fee_deduction).
//! Решение 217: freeze — снимок стакана и плана комиссии (gateway/frozen.rs FrozenBook) для контракта quote_snapshot;
//! живая котировка — тот же снимок на один вызов (одна формула).
//! Кто вызывает: ws_conn.rs (кладёт стакан), mod.rs (quote для ядра, book_confirmed), book_print.rs (print).
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
//! Решение 213 (по содержимому, Тигран 08.11): тихая пара, у которой сторож ядра подтвердил по REST «первые N уровней
//! стакана на бирже совпадают со стаканом в памяти» (book_print.rs, контракт book_confirmed), — свежая до срока
//! подтверждения, даже если кадра нет дольше venue.book_stale_ms. Живость соединения подтверждение не отменяет. Новый
//! стакан из кадра (put) или снятие стакана — подтверждения нет (как у MEXC).

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
        fees: &Fees,
        asset: &Asset,
        side: Side,
        qty: Decimal,
    ) -> GatewayResult<Quote> {
        Ok(self.freeze(ctx, fees, asset, side)?.price(qty))
    }

    /// Решение 217: снимок стакана монеты и плана комиссии на этот момент (копия стакана из памяти).
    /// Принимает: то же, что quote, без количества. Возвращает: FrozenBook.
    /// Ошибки: StaleData — стакана нет, соединение молчит или кадра пары нет дольше venue.book_stale_ms и нет
    /// подтверждения тихой пары (решение 213), ставки нет; NotConfigured — настройка не задана или вариант комиссии
    /// неизвестен.
    pub fn freeze(
        &self,
        ctx: &Ctx,
        fees: &Fees,
        asset: &Asset,
        side: Side,
    ) -> GatewayResult<FrozenBook> {
        let stale_ms = ctx.int("feed.stale_after_ms")? as i64;
        let pair_ms = ctx.int("venue.book_stale_ms")? as i64;
        let quote_asset = ctx.text("venue.quote_asset")?;
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
        let rates = fees.get(ctx, &ctx.symbol(&asset.ticker)?)?;
        let mode = ctx.text("venue.fee_deduction")?;
        // Решение 158: скидка — только если монеты скидки хватает (память discount_balance.rs).
        let (discount_ok, _) = fees.discount.ok(ctx);
        let (rate, in_discount, _why) = rates
            .plan(&mode, side, discount_ok)
            .map_err(|m| ctx.tag(GatewayError::not_set(None, "venue.fee_deduction", m)))?;
        // Покупка без скидки — комиссия в монете от количества, иначе — по сумме сделки. Решение 219: со скидкой
        // комиссия берётся монетой скидки (discountAsset — BNB, faqs/commission_faq): сумма — эквивалент в котируемой
        // монете по ставке со скидкой, платится монетой скидки (paid_in) — не из котируемой и не из монеты сделки.
        let fee = if side == Side::Buy && !in_discount {
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
                paid_in: in_discount.then(|| rates.discount_asset.clone()),
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
}

/// Проход по уровням от лучшего — общий для бирж (gateway/frozen.rs, решение 217).
pub use crate::gateway::frozen::walk;

/// Ошибка «данные устарели». Ошибок нет.
fn stale(ctx: &Ctx, ticker: &str, why: &str) -> GatewayError {
    ctx.tag(GatewayError::new(
        ErrorCode::StaleData,
        None,
        format!("Binance {ticker}: {why} — котировки нет"),
    ))
}
