//! Файл: venues/poloniex/symbols.rs
//! Что делает: правила торговли пар Poloniex (шаги цены и количества из priceScale / quantityScale, минимумы и максимумы
//! количества и суммы, границы цены, состояние пары) — справочник в памяти с обновлением по запросу.
//! Кто вызывает: order_rules.rs (правила ордера), pairs.rs (состояние пар для ядра), pairs_watch.rs, mod.rs.
//! От чего зависит: rest.rs, symbols_parse.rs (разбор), ctx.rs.
//! Настройки: venue.quote_asset, venue.symbols_refresh_s [poloniex].
//!
//! Официально — Symbol Information GET /markets («30/с» по IP, Spot → API → Rate Limits)
//! https://api-docs.poloniex.com/spot/api/public/reference-data; поля — в шапке symbols_parse.rs. Флагов «покупка/
//! продажа через API» у Poloniex нет — только state: торгуется лишь NORMAL («PAUSE» — пауза, «POST_ONLY» — только
//! ордера LIMIT_MAKER, наш IOC не пройдёт — код 21341).

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::events::SpotPairState;
use crate::storage::journal::Level;

use super::ctx::Ctx;
use super::rest;
pub use super::symbols_parse::parse_list;

/// Состояние пары (свои коды для ядра): NORMAL — торгуется.
pub const STATUS_NORMAL: i64 = 1;
/// PAUSE — торговля приостановлена.
pub const STATUS_PAUSE: i64 = 2;
/// POST_ONLY — только LIMIT_MAKER (ордер IOC не пройдёт).
pub const STATUS_POST_ONLY: i64 = 3;
/// Значение state вне перечня документации (пара не торгуется ботом).
pub const STATUS_UNKNOWN: i64 = 9;

/// Правила одной пары.
#[derive(Debug, Clone, PartialEq)]
pub struct SymbolRules {
    /// Символ Poloniex, например CAKE_USDT.
    pub symbol: String,
    /// Базовая и котируемая монеты (baseCurrencyName, quoteCurrencyName).
    pub base: String,
    pub quote: String,
    /// Шаг цены = 10^−priceScale.
    pub tick_size: Decimal,
    /// Шаг количества = 10^−quantityScale.
    pub step_size: Decimal,
    /// Наименьшее и наибольшее количество (minQuantity, maxQuantity; «0» у максимума — «no check» → None).
    pub min_qty: Option<Decimal>,
    pub max_qty: Option<Decimal>,
    /// Наименьшая и наибольшая сумма в котируемой монете (minAmount, maxAmount; «0» у максимума → None).
    pub min_notional: Option<Decimal>,
    pub max_notional: Option<Decimal>,
    /// Границы цены: покупка не дороже highestBid, продажа не дешевле lowestAsk («market bound»). None — поля нет или
    /// оно не больше нуля (смысл нуля в документации не описан — проверить на живом ответе).
    pub highest_bid: Option<Decimal>,
    pub lowest_ask: Option<Decimal>,
    /// STATUS_*.
    pub status: i64,
}

impl SymbolRules {
    /// Пара торгуется через API (в обе стороны одинаково — запрета стороны у Poloniex нет). Ошибок нет.
    pub fn tradable(&self) -> bool {
        self.status == STATUS_NORMAL
    }
}

/// Когда справочник получен: для срока (Instant) и для ядра (время).
type Loaded = Option<(Instant, DateTime<Utc>)>;

/// Справочник пар (ключ — тикер монеты). Дешёво клонируется; правила — за Arc (без копии всего справочника).
#[derive(Clone, Default)]
pub struct Symbols {
    map: Arc<RwLock<Arc<HashMap<String, SymbolRules>>>>,
    loaded: Arc<RwLock<Loaded>>,
    told: Arc<RwLock<String>>,
}

impl Symbols {
    /// Загружает все пары с котируемой монетой venue.quote_asset. Возвращает: число пар.
    /// Ошибки: связь, ответ Poloniex, настройки. Пары с неразборчивыми полями — в журнал (при изменении списка).
    pub async fn refresh(&self, ctx: &Ctx) -> GatewayResult<usize> {
        let quote = ctx.text("venue.quote_asset")?;
        let data = rest::public(ctx, "/markets", vec![]).await?;
        let (map, skipped) = parse_list(&data, &quote)?;
        let n = map.len();
        self.load(map, Utc::now());
        let text = skipped.join("; ");
        let new = {
            let mut t = self.told.write().unwrap_or_else(|p| p.into_inner());
            let new = *t != text;
            *t = text.clone();
            new
        };
        if new && !skipped.is_empty() {
            let m = format!(
                "symbols: пропущено пар с неразборчивыми полями: {} — {text}",
                skipped.len()
            );
            ctx.log(Level::Warn, "venue.symbols", m).await;
        }
        Ok(n)
    }

    /// Ставит разобранный справочник и время получения. Ошибок нет.
    pub fn load(&self, map: HashMap<String, SymbolRules>, at: DateTime<Utc>) {
        *self.map.write().unwrap_or_else(|p| p.into_inner()) = Arc::new(map);
        *self.loaded.write().unwrap_or_else(|p| p.into_inner()) = Some((Instant::now(), at));
    }

    /// Когда справочник последний раз успешно получен. Ошибок нет.
    pub fn loaded_at(&self) -> Option<DateTime<Utc>> {
        self.loaded
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .map(|(_, at)| at)
    }

    /// Перечитывает справочник, если он не загружен или старше venue.symbols_refresh_s. Ошибки: как у refresh.
    pub async fn ensure_fresh(&self, ctx: &Ctx) -> GatewayResult<()> {
        let max_age = Duration::from_secs(ctx.int("venue.symbols_refresh_s")?);
        let loaded = *self.loaded.read().unwrap_or_else(|p| p.into_inner());
        if loaded.is_none_or(|(at, _)| at.elapsed() >= max_age) {
            self.refresh(ctx).await?;
        }
        Ok(())
    }

    /// Весь справочник (указатель, без копии). Ошибок нет.
    pub fn all(&self) -> Arc<HashMap<String, SymbolRules>> {
        self.map.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Правила пары по тикеру. Ошибка BadResponse — такой пары у Poloniex нет.
    pub fn get(&self, ctx: &Ctx, ticker: &str) -> GatewayResult<SymbolRules> {
        self.all().get(ticker).cloned().ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::BadResponse,
                None,
                format!("Пары {ticker} нет в справочнике Poloniex"),
            ))
        })
    }

    /// Символ Poloniex пары монеты. None — пары нет. Ошибок нет.
    pub fn symbol_of(&self, ticker: &str) -> Option<String> {
        self.all().get(ticker).map(|r| r.symbol.clone())
    }

    /// Состояние пары монеты для ядра (MarketEvent::SpotPairs). None — пары нет. Ошибок нет.
    pub fn state(&self, ticker: &str) -> Option<SpotPairState> {
        self.all().get(ticker).map(|r| SpotPairState {
            ticker: ticker.to_string(),
            status: r.status,
            online: r.tradable(),
            api_buy: r.tradable(),
            api_sell: r.tradable(),
            step_size: r.step_size,
            min_notional: r.min_notional,
            max_notional: r.max_notional,
            min_qty: r.min_qty,
            max_qty: r.max_qty,
        })
    }
}
