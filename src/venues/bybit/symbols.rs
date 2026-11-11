//! Файл: venues/bybit/symbols.rs
//! Что делает: правила торговли спот-пар Bybit (шаг цены, шаг количества, минимальная сумма, наибольшее количество
//! лимитного ордера, статус) — справочник в памяти с обновлением по запросу.
//! Кто вызывает: order_rules.rs (правила ордера), pairs.rs (состояние пар для ядра), pairs_watch.rs, wallet.rs.
//! От чего зависит: rest.rs, symbols_parse.rs (разбор), ctx.rs.
//! Настройки: venue.quote_asset, venue.symbols_refresh_s.
//!
//! Официальная документация: GET /v5/market/instruments-info?category=spot (публичный)
//! https://bybit-exchange.github.io/docs/v5/market/instrument — поля в шапке symbols_parse.rs. Признака «торгуется
//! через API» у пары в документации нет (есть только отказ 170157 «The trading pair is not available for api trading»):
//! покупка и продажа через API считаются открытыми у пары со статусом Trading; отказ биржи — OrderRejected.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::events::SpotPairState;

use super::ctx::Ctx;
use super::rest;
pub use super::symbols_parse::parse_list;

/// Код статуса «Trading» для ядра (SpotPairState.status — целое). Коды — порядок перечня status в документации
/// (enum: PreLaunch, PendingOpen, Trading, Delivering, Closed); неизвестный — −1.
pub const STATUS_TRADING: i64 = 3;

/// Строка статуса Bybit → код ядра. Ошибок нет.
pub fn status_code(s: &str) -> i64 {
    match s {
        "PreLaunch" => 1,
        "PendingOpen" => 2,
        "Trading" => STATUS_TRADING,
        "Delivering" => 4,
        "Closed" => 5,
        _ => -1,
    }
}

/// Правила одной пары.
#[derive(Debug, Clone, PartialEq)]
pub struct SymbolRules {
    /// Символ Bybit, например CAKEUSDT.
    pub symbol: String,
    pub tick_size: Decimal,
    pub step_size: Decimal,
    /// minOrderAmt — наименьшая сумма ордера в котируемой монете.
    pub min_notional: Option<Decimal>,
    /// maxLimitOrderQty — наибольшее количество лимитного ордера.
    pub max_qty: Option<Decimal>,
    /// Код статуса (status_code) и сама строка Bybit.
    pub status: i64,
    pub status_text: String,
}

impl SymbolRules {
    /// Пара торгуется (статус Trading). Ошибок нет.
    pub fn tradable(&self) -> bool {
        self.status == STATUS_TRADING
    }
}

/// Когда справочник получен: момент процесса (для возраста) и время по часам.
type Loaded = (Instant, DateTime<Utc>);

/// Справочник пар (ключ — тикер монеты). Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Symbols {
    map: Arc<RwLock<HashMap<String, SymbolRules>>>,
    loaded: Arc<RwLock<Option<Loaded>>>,
}

impl Symbols {
    /// Загружает все спот-пары с котируемой монетой venue.quote_asset. Возвращает: число пар.
    /// Ошибки: связь, ответ Bybit, настройки.
    pub async fn refresh(&self, ctx: &Ctx) -> GatewayResult<usize> {
        let quote = ctx.text("venue.quote_asset")?;
        let p = vec![("category", "spot".to_string())];
        let info = rest::public(ctx, "/v5/market/instruments-info", p).await?;
        let map = parse_list(&info, &quote)?;
        let n = map.len();
        self.load(map, Utc::now());
        Ok(n)
    }

    /// Ставит разобранный справочник и время получения. Ошибок нет.
    pub fn load(&self, map: HashMap<String, SymbolRules>, at: DateTime<Utc>) {
        *self.map.write().unwrap_or_else(|p| p.into_inner()) = map;
        *self.loaded.write().unwrap_or_else(|p| p.into_inner()) = Some((Instant::now(), at));
    }

    /// Когда справочник последний раз успешно получен. Ошибок нет.
    pub fn loaded_at(&self) -> Option<DateTime<Utc>> {
        self.loaded
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .map(|(_, at)| at)
    }

    /// Перечитывает справочник, если он не загружен или старше venue.symbols_refresh_s. Ошибки: как refresh.
    pub async fn ensure_fresh(&self, ctx: &Ctx) -> GatewayResult<()> {
        let max_age = Duration::from_secs(ctx.int("venue.symbols_refresh_s")?);
        let loaded = *self.loaded.read().unwrap_or_else(|p| p.into_inner());
        if loaded.is_none_or(|(at, _)| at.elapsed() >= max_age) {
            self.refresh(ctx).await?;
        }
        Ok(())
    }

    /// Правила пары по тикеру. Ошибка BadResponse — такой пары у Bybit нет.
    pub fn get(&self, ctx: &Ctx, ticker: &str) -> GatewayResult<SymbolRules> {
        let map = self.map.read().unwrap_or_else(|p| p.into_inner());
        map.get(ticker).cloned().ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::BadResponse,
                None,
                format!("Пары {ticker} нет в справочнике спота Bybit"),
            ))
        })
    }

    /// Состояние пары монеты для ядра (MarketEvent::SpotPairs). None — пары нет. Ошибок нет.
    pub fn state(&self, ticker: &str) -> Option<SpotPairState> {
        let map = self.map.read().unwrap_or_else(|p| p.into_inner());
        map.get(ticker).map(|r| SpotPairState {
            ticker: ticker.to_string(),
            status: r.status,
            online: r.tradable(),
            api_buy: r.tradable(),
            api_sell: r.tradable(),
            step_size: r.step_size,
            min_notional: r.min_notional,
            max_notional: None,
            min_qty: None,
            max_qty: r.max_qty,
            order_block: None,
        })
    }

    /// Символ Bybit пары монеты. None — пары нет. Ошибок нет.
    pub fn symbol_of(&self, ticker: &str) -> Option<String> {
        let map = self.map.read().unwrap_or_else(|p| p.into_inner());
        map.get(ticker).map(|r| r.symbol.clone())
    }
}
