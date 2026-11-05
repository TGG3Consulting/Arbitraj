//! Файл: venues/binance/symbols.rs
//! Что делает: правила торговли пар Binance (шаг цены и количества, пределы цены, количества и суммы, статус, можно ли
//! торговать через API нашему аккаунту) — справочник в памяти с обновлением по запросу.
//! Кто вызывает: order_rules.rs, trade.rs, pairs.rs, pairs_watch.rs, market.rs, tap.rs.
//! От чего зависит: rest.rs, symbols_parse.rs (разбор), ctx.rs.
//! Настройки: venue.quote_asset, venue.symbols_refresh_s.
//!
//! Официально: GET /api/v3/exchangeInfo (вес 20, rest-api «Exchange information») — status (перечень Symbol status в
//! enums.md: PRE_TRADING, TRADING, POST_TRADING, END_OF_DAY, HALT, AUCTION_MATCH, BREAK), фильтры (filters.md);
//! GET /api/v3/account (вес 20) — permissions аккаунта для permissionSets пары. Шаги у Binance явные (tickSize,
//! stepSize) — толкования precision, как у MEXC, нет. Справочник — тысячи пар: разбор вне рабочих потоков (решение 131).

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::events::SpotPairState;

use super::ctx::Ctx;
use super::rest;
use super::symbols_parse::{account_permissions, parse_list};

/// Статусы пары из перечня enums.md по порядку; код — номер в перечне (TRADING = 2). Код нужен ядру для показа
/// «пара не торгуется (status N)»; имя — status_name. Это не настройка: это перечень документации.
const STATUSES: [&str; 7] = [
    "PRE_TRADING",
    "TRADING",
    "POST_TRADING",
    "END_OF_DAY",
    "HALT",
    "AUCTION_MATCH",
    "BREAK",
];
/// Код статуса «пара торгуется» (TRADING).
pub const STATUS_TRADING: i64 = 2;

/// Код статуса по имени (1-based номер в перечне; неизвестный — −1). Ошибок нет.
pub fn status_code(s: &str) -> i64 {
    STATUSES
        .iter()
        .position(|x| *x == s)
        .map_or(-1, |i| i as i64 + 1)
}

/// Имя статуса по коду (для журнала). Ошибок нет.
pub fn status_name(code: i64) -> &'static str {
    usize::try_from(code - 1)
        .ok()
        .and_then(|i| STATUSES.get(i).copied())
        .unwrap_or("неизвестный")
}

/// Правила одной пары.
#[derive(Debug, Clone)]
pub struct SymbolRules {
    /// Символ Binance, например CAKEUSDT.
    pub symbol: String,
    /// Шаги (PRICE_FILTER.tickSize, LOT_SIZE.stepSize); 0 — правило выключено.
    pub tick_size: Decimal,
    pub step_size: Decimal,
    pub min_price: Option<Decimal>,
    pub max_price: Option<Decimal>,
    pub min_qty: Option<Decimal>,
    pub max_qty: Option<Decimal>,
    pub min_notional: Option<Decimal>,
    pub max_notional: Option<Decimal>,
    /// Код статуса (status_code) и имя как в ответе.
    pub status: i64,
    pub status_text: String,
    /// Можно купить / продать через API: isSpotTradingAllowed и права аккаунта покрывают permissionSets.
    pub api_buy: bool,
    pub api_sell: bool,
    /// Типы ордера пары (orderTypes); None — поля нет (тогда тип проверяет только пробный ордер).
    pub order_types: Option<Vec<String>>,
}

impl SymbolRules {
    /// Пара торгуется через API в указанную сторону. Ошибок нет.
    pub fn tradable(&self, buy: bool) -> bool {
        self.status == STATUS_TRADING && if buy { self.api_buy } else { self.api_sell }
    }
}

/// Когда справочник получен: для срока (Instant) и для ядра (время UTC).
type Loaded = Option<(Instant, DateTime<Utc>)>;

/// Справочник пар (ключ — тикер монеты). Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Symbols {
    map: Arc<RwLock<Arc<HashMap<String, SymbolRules>>>>,
    loaded: Arc<RwLock<Loaded>>,
}

impl Symbols {
    /// Загружает пары с котируемой монетой venue.quote_asset и права аккаунта. Возвращает: число пар.
    /// Ошибки: связь, ответ Binance, настройки, ключи (account — подписанный).
    pub async fn refresh(&self, ctx: &Ctx) -> GatewayResult<usize> {
        let quote = ctx.text("venue.quote_asset")?;
        let account = super::account::info(ctx).await?;
        let perms = account_permissions(&account)?;
        let info = rest::public_heavy(ctx, "/api/v3/exchangeInfo", vec![]).await?;
        let c = ctx.clone();
        let map = tokio::task::spawn_blocking(move || parse_list(&info, &quote, &perms))
            .await
            .map_err(|e| {
                c.tag(GatewayError::new(
                    ErrorCode::Internal,
                    None,
                    format!("разбор exchangeInfo прерван: {e}"),
                ))
            })??;
        let n = map.len();
        self.load(map, Utc::now());
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

    /// Справочник целиком (указатель, без копии). Ошибок нет.
    fn all(&self) -> Arc<HashMap<String, SymbolRules>> {
        self.map.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Правила пары по тикеру. Ошибка BadResponse — такой пары у Binance нет.
    pub fn get(&self, ctx: &Ctx, ticker: &str) -> GatewayResult<SymbolRules> {
        self.all().get(ticker).cloned().ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::BadResponse,
                None,
                format!("Пары {ticker} нет в справочнике Binance"),
            ))
        })
    }

    /// Состояние пары монеты для ядра (MarketEvent::SpotPairs). None — пары нет. Ошибок нет.
    pub fn state(&self, ticker: &str) -> Option<SpotPairState> {
        self.all().get(ticker).map(|r| SpotPairState {
            ticker: ticker.to_string(),
            status: r.status,
            online: r.status == STATUS_TRADING,
            api_buy: r.api_buy,
            api_sell: r.api_sell,
            step_size: r.step_size,
            min_notional: r.min_notional,
            max_notional: r.max_notional,
            min_qty: r.min_qty,
            max_qty: r.max_qty,
        })
    }

    /// Символ Binance пары монеты. None — пары нет. Ошибок нет.
    pub fn symbol_of(&self, ticker: &str) -> Option<String> {
        self.all().get(ticker).map(|r| r.symbol.clone())
    }
}
