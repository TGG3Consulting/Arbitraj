//! Файл: venues/htx/book.rs
//! Что делает: последние стаканы HTX в памяти и котировка на объём по уровням стакана
//! (сколько заплатим или получим за Q монет, с худшей комиссией тейкера нашего ключа — fees.rs).
//! Решение 217: freeze — снимок стакана и плана комиссии (gateway/frozen.rs FrozenBook) для контракта quote_snapshot;
//! живая котировка — тот же снимок на один вызов (одна формула).
//! Кто вызывает: market.rs (обновляет стакан), mod.rs (quote для ядра).
//! От чего зависит: core/types, fees.rs, ctx.rs.
//! Настройки: feed.stale_after_ms (по площадке) — если соединение, по которому пришёл стакан, молчит дольше, данные
//! устарели; venue.book_stale_ms [htx] (необязательна) — кадр стакана этой пары старше — котировки нет;
//! venue.quote_asset [htx] — монета комиссии продажи.
//!
//! Свежесть: живость соединения (любой кадр, включая ping сервера) и, если задана venue.book_stale_ms, время
//! последнего кадра стакана самой пары (кадр с тем же seqNum тоже обновляет время — ws_conn.rs, решение 109). Шлёт ли
//! HTX снимок mbp.refresh без изменений рынка, документация не говорит («The update interval is around 100ms») —
//! проверить на живой записи; до того venue.book_stale_ms задавать с запасом.
//! Комиссия: покупка — в купленной монете (её становится меньше), продажа — в котируемой (как списывает HTX без
//! скидки); со скидкой HT факт только меньше (fees.rs).

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

    /// Котировка на объём (снимок на один вызов — та же формула, что у снимка решения 217).
    /// Принимает: окружение, данные комиссии, монету, сторону, количество монет.
    /// Возвращает: Quote (fillable_qty меньше qty, если глубины не хватает). Ошибки: как у freeze.
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

    /// Решение 217: снимок стакана монеты и плана комиссии на этот момент (копия стакана из памяти).
    /// Принимает: то же, что quote, без количества. Возвращает: FrozenBook.
    /// Ошибки: StaleData — стакана нет, соединение молчит дольше feed.stale_after_ms или (если задана)
    /// кадра пары нет дольше venue.book_stale_ms; NotConfigured — feed.stale_after_ms не задана; комиссия.
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
        let rates = fees.get(ctx, symbol)?;
        // Покупка — комиссия в монете от количества, продажа — в котируемой монете от суммы.
        let fee = if side == Side::Buy {
            BookFee {
                rate: rates.taker,
                per_coin: true,
                asset: asset.ticker.clone(),
                paid_in: None,
            }
        } else {
            BookFee {
                rate: rates.taker,
                per_coin: false,
                asset: ctx.text("venue.quote_asset")?,
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

/// Ошибка «данные устарели».
fn stale(ctx: &Ctx, ticker: &str, why: &str) -> GatewayError {
    ctx.tag(GatewayError::new(
        ErrorCode::StaleData,
        None,
        format!("HTX {ticker}: {why} — котировки нет"),
    ))
}
