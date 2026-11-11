//! Файл: venues/coinex/symbols.rs
//! Что делает: правила торговли пар CoinEx (шаг цены и количества из числа знаков, минимальное количество, статус пары,
//! торговля через API) — справочник в памяти с обновлением по запросу.
//! Кто вызывает: order_rules.rs (правила ордера), pairs.rs (состояние пар для ядра), pairs_watch.rs, mod.rs.
//! От чего зависит: rest.rs, symbols_parse.rs (разбор), ctx.rs.
//! Настройки: venue.quote_asset, venue.symbols_refresh_s [coinex].
//!
//! Официально — Get Market Status GET /spot/market (без market — «all markets are returned»)
//! https://docs.coinex.com/api/v2/spot/market/http/list-market; поля — в шапке symbols_parse.rs. Статусы пары
//! (market_status, https://docs.coinex.com/api/v2/enum): online «available», counting_down, bidding; торговля через API —
//! is_api_trading_available. Торгуется ботом только online и с API-торговлей.

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

/// Статус пары (свои коды для ядра): online — «available».
pub const STATUS_ONLINE: i64 = 1;
/// counting_down — «counting down».
pub const STATUS_COUNTING_DOWN: i64 = 2;
/// bidding — «bidding».
pub const STATUS_BIDDING: i64 = 3;
/// Значение status вне перечня документации (пара не торгуется ботом).
pub const STATUS_UNKNOWN: i64 = 9;

/// Правила одной пары.
#[derive(Debug, Clone, PartialEq)]
pub struct SymbolRules {
    /// Имя рынка CoinEx (market), например CAKEUSDT.
    pub symbol: String,
    /// Базовая и котируемая монеты (base_ccy, quote_ccy).
    pub base: String,
    pub quote: String,
    /// Шаг цены = 10^−quote_ccy_precision («Decimal places for the quote currency»).
    pub tick_size: Decimal,
    /// Шаг количества = 10^−base_ccy_precision («Decimal places for the base currency»).
    pub step_size: Decimal,
    /// Минимальное количество — min_amount («Minimum transaction volume»; в базовой монете — проверить на живом ордере).
    pub min_qty: Option<Decimal>,
    /// STATUS_*.
    pub status: i64,
    /// is_api_trading_available — «Whether API trading is enabled».
    pub api: bool,
}

impl SymbolRules {
    /// Пара торгуется через API: status online и is_api_trading_available (запрета одной стороны у CoinEx нет).
    /// Ошибок нет.
    pub fn tradable(&self) -> bool {
        self.status == STATUS_ONLINE && self.api
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
    /// Ошибки: связь, ответ CoinEx, настройки. Пары с неразборчивыми полями — в журнал (при изменении списка).
    pub async fn refresh(&self, ctx: &Ctx) -> GatewayResult<usize> {
        let quote = ctx.text("venue.quote_asset")?;
        let data = rest::public(ctx, "/v2/spot/market", vec![]).await?;
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

    /// Правила пары по тикеру. Ошибка BadResponse — такой пары у CoinEx нет.
    pub fn get(&self, ctx: &Ctx, ticker: &str) -> GatewayResult<SymbolRules> {
        self.all().get(ticker).cloned().ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::BadResponse,
                None,
                format!("Пары {ticker} нет в справочнике CoinEx"),
            ))
        })
    }

    /// Символ CoinEx пары монеты. None — пары нет. Ошибок нет.
    pub fn symbol_of(&self, ticker: &str) -> Option<String> {
        self.all().get(ticker).map(|r| r.symbol.clone())
    }

    /// Состояние пары монеты для ядра (MarketEvent::SpotPairs). None — пары нет. Ошибок нет.
    pub fn state(&self, ticker: &str) -> Option<SpotPairState> {
        self.all().get(ticker).map(|r| SpotPairState {
            ticker: ticker.to_string(),
            status: r.status,
            online: r.status == STATUS_ONLINE,
            api_buy: r.tradable(),
            api_sell: r.tradable(),
            step_size: r.step_size,
            min_notional: None,
            max_notional: None,
            min_qty: r.min_qty,
            max_qty: None,
            order_block: None,
        })
    }
}
