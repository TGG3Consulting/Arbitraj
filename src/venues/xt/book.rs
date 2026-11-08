//! Файл: venues/xt/book.rs
//! Что делает: последние стаканы XT.com в памяти и котировка на объём по уровням стакана (сколько заплатим или
//! получим за Q монет) с комиссией тейкера пары (takerFeeRate справочника пар — ставки по ключу у XT нет).
//! Решение 217: freeze — снимок стакана и плана комиссии (gateway/frozen.rs FrozenBook) для контракта quote_snapshot;
//! живая котировка — тот же снимок на один вызов (одна формула).
//! Кто вызывает: ws_conn.rs (кладёт стакан), mod.rs (quote для ядра).
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
    /// Ошибки: StaleData — стакана нет, соединение молчит или кадра пары нет дольше venue.book_stale_ms, ставки нет;
    /// NotConfigured — настройка не задана.
    pub fn freeze(
        &self,
        ctx: &Ctx,
        symbols: &Symbols,
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
        let rate = symbols
            .taker_fee(&asset.ticker)
            .ok_or_else(|| stale(ctx, &asset.ticker, "XT не сообщил takerFeeRate пары"))?;
        // Покупка — комиссия в монете от количества, продажа — в котируемой монете от суммы.
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
