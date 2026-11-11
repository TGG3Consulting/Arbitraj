//! Файл: venues/mexc/symbols.rs
//! Что делает: правила торговли пар MEXC (шаг цены, шаг количества, минимальное количество и сумма, статус, разрешены
//! ли покупка и продажа через API нашему ключу) — справочник в памяти с обновлением по запросу.
//! Кто вызывает: trade.rs (округление и проверки перед ордером), pairs.rs (состояние пар для ядра), pairs_watch.rs.
//! От чего зависит: rest.rs, symbols_parse.rs (разбор ответов), ctx.rs, order_rules.rs (признак «не примет наш
//! ордер» — решение 254).
//! Настройки: venue.quote_asset, venue.symbols_refresh_s.
//!
//! Официальная документация:
//! - GET /api/v3/exchangeInfo https://www.mexc.com/api-docs/spot-v3/market-data-endpoints/exchange-information —
//!   status «1 - online, 2 - Pause, 3 - offline» (в старом примере — «ENABLED»: принимаются оба), isSpotTradingAllowed
//!   «allow api spot trading», tradeSideType «1 - All, 2 - buy order only, 3 - Sell order only, 4 - Close»,
//!   baseSizePrecision «min order quantity», quoteAmountPrecision «min order amount», maxQuoteAmount,
//!   baseAssetPrecision / quotePrecision — целые «base Asset Precision» / «quote Precision».
//! - GET /api/v3/selfSymbols (подписанный, вес 1) https://www.mexc.com/api-docs/spot-v3/spot-account-trade/user-api-default-symbol
//!   — пары, которыми наш ключ может торговать через API (задаются в кабинете: «My API Key — Trading Pairs»).
//!
//! Шага цены и количества (tickSize/stepSize) в документации НЕТ (docs/notes/mexc-analysis.md). Решение 107: шаг
//! количества = 10^−baseAssetPrecision, шаг цены = 10^−quotePrecision — толкование целых precision как числа знаков
//! после запятой. Ошибка толкования ловится до денег: перед каждым ордером — пробный ордер POST /api/v3/order/test
//! (trade.rs, настройка venue.order_test_first [mexc]); отказ — ордер не ставится.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use reqwest::Method;
use rust_decimal::Decimal;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::events::SpotPairState;

use super::ctx::Ctx;
use super::errors::Op;
use super::rest;
pub use super::symbols_parse::{parse_list, self_symbols};

/// Статус «пара торгуется» — по официальной документации («1 - online»).
pub const STATUS_ONLINE: i64 = 1;

/// Правила одной пары.
#[derive(Debug, Clone)]
pub struct SymbolRules {
    /// Символ MEXC, например CAKEUSDT.
    pub symbol: String,
    pub tick_size: Decimal,
    pub step_size: Decimal,
    pub min_notional: Option<Decimal>,
    pub max_notional: Option<Decimal>,
    pub min_qty: Option<Decimal>,
    /// Статус: 1 online, 2 pause, 3 offline (строка «ENABLED» старого примера → 1); иное — −1.
    pub status: i64,
    /// Можно купить / продать через API: isSpotTradingAllowed, пара в selfSymbols ключа, сторона не закрыта tradeSideType.
    pub api_buy: bool,
    pub api_sell: bool,
    /// Знаков после запятой у количества монеты (baseAssetPrecision) и у суммы котируемой монеты (quoteAssetPrecision).
    pub base_precision: u32,
    pub quote_asset_precision: u32,
    /// Допустимые типы ордера пары (orderTypes); None — поля нет в ответе (тогда проверяет только пробный ордер).
    pub order_types: Option<Vec<String>>,
}

impl SymbolRules {
    /// Пара торгуется через API в указанную сторону.
    pub fn tradable(&self, buy: bool) -> bool {
        self.status == STATUS_ONLINE && if buy { self.api_buy } else { self.api_sell }
    }

    /// Можно ли ставить ордер типа `t` на этой паре (решение 133). Типы IMMEDIATE_OR_CANCEL и FILL_OR_KILL есть в
    /// перечне типов ордера документации (https://www.mexc.com/api-docs/spot-v3/public-api-definitions), но в
    /// orderTypes пар exchangeInfo MEXC их не пишет ни у одной пары (живой ответ 03.11: даже BTCUSDT — LIMIT, MARKET,
    /// LIMIT_MAKER). Живой факт: 02.11 21:31 сделка 20261102213158008XPL — покупка ордером IMMEDIATE_OR_CANCEL принята и
    /// исполнена. IOC и FOK — лимитный ордер с правилом снятия остатка, поэтому допустимы, когда у пары есть LIMIT;
    /// окончательно тип подтверждает пробный ордер /api/v3/order/test (venue.order_test_first). Остальные типы — только
    /// из orderTypes. Поля orderTypes нет — проверяет только пробный ордер (true). Ошибок нет.
    pub fn order_type_allowed(&self, t: &str) -> bool {
        let Some(types) = &self.order_types else {
            return true;
        };
        let need = match t {
            "IMMEDIATE_OR_CANCEL" | "FILL_OR_KILL" => "LIMIT",
            other => other,
        };
        types.iter().any(|x| x == need)
    }
}

/// Справочник пар (ключ — тикер монеты). Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Symbols {
    map: Arc<RwLock<HashMap<String, SymbolRules>>>,
    loaded: Arc<RwLock<Option<Instant>>>,
    loaded_at: Arc<RwLock<Option<DateTime<Utc>>>>,
}

impl Symbols {
    /// Загружает все пары с котируемой монетой venue.quote_asset и разрешённые ключу пары.
    /// Возвращает: число пар. Ошибки: связь, ответ MEXC, настройки, ключи (selfSymbols — подписанный).
    pub async fn refresh(&self, ctx: &Ctx) -> GatewayResult<usize> {
        let quote = ctx.text("venue.quote_asset")?;
        let info = rest::public(ctx, "/api/v3/exchangeInfo", vec![]).await?;
        let own = rest::signed(ctx, Method::GET, "/api/v3/selfSymbols", vec![], Op::Other).await?;
        let allowed = self_symbols(&own)?;
        let map = parse_list(&info, &quote, &allowed)?;
        let n = map.len();
        self.load(map, Utc::now());
        Ok(n)
    }

    /// Ставит разобранный справочник и время получения. Ошибок нет.
    pub fn load(&self, map: HashMap<String, SymbolRules>, at: DateTime<Utc>) {
        *self.map.write().unwrap_or_else(|p| p.into_inner()) = map;
        *self.loaded.write().unwrap_or_else(|p| p.into_inner()) = Some(Instant::now());
        *self.loaded_at.write().unwrap_or_else(|p| p.into_inner()) = Some(at);
    }

    /// Когда справочник последний раз успешно получен. Ошибок нет.
    pub fn loaded_at(&self) -> Option<DateTime<Utc>> {
        *self.loaded_at.read().unwrap_or_else(|p| p.into_inner())
    }

    /// Перечитывает справочник, если он не загружен или старше venue.symbols_refresh_s. Ошибки: как у refresh.
    pub async fn ensure_fresh(&self, ctx: &Ctx) -> GatewayResult<()> {
        let max_age = Duration::from_secs(ctx.int("venue.symbols_refresh_s")?);
        let loaded = *self.loaded.read().unwrap_or_else(|p| p.into_inner());
        if loaded.is_none_or(|at| at.elapsed() >= max_age) {
            self.refresh(ctx).await?;
        }
        Ok(())
    }

    /// Правила пары по тикеру. Ошибка BadResponse — такой пары у MEXC нет.
    pub fn get(&self, ctx: &Ctx, ticker: &str) -> GatewayResult<SymbolRules> {
        let map = self.map.read().unwrap_or_else(|p| p.into_inner());
        map.get(ticker).cloned().ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::BadResponse,
                None,
                format!("Пары {ticker} нет в справочнике MEXC"),
            ))
        })
    }

    /// Состояние пары монеты для ядра (MarketEvent::SpotPairs). Принимает: тикер, тип ордера из настройки
    /// venue.order_type (None — не задан) — для признака «MEXC не примет наш ордер» (решение 254,
    /// order_rules::order_block). None — пары нет. Ошибок нет.
    pub fn state(&self, ticker: &str, order_type: Option<&str>) -> Option<SpotPairState> {
        let map = self.map.read().unwrap_or_else(|p| p.into_inner());
        map.get(ticker).map(|r| SpotPairState {
            ticker: ticker.to_string(),
            status: r.status,
            online: r.status == STATUS_ONLINE,
            api_buy: r.api_buy,
            api_sell: r.api_sell,
            step_size: r.step_size,
            min_notional: r.min_notional,
            max_notional: r.max_notional,
            min_qty: r.min_qty,
            max_qty: None,
            order_block: super::order_rules::order_block(r, order_type),
        })
    }

    /// Точность вывода монеты (решение 107): у монеты пары — baseAssetPrecision; у котируемой монеты (USDT) —
    /// quoteAssetPrecision её пар (наименьшая — безопасно). Котируемая — только точное совпадение: символ пары равен
    /// «база + тикер» (решение 118, А-🟡15; раньше ends_with — у T, DT, SDT бралась чужая точность). None — неизвестно.
    /// Ошибок нет.
    pub fn precision_of(&self, ticker: &str) -> Option<u32> {
        let map = self.map.read().unwrap_or_else(|p| p.into_inner());
        if let Some(r) = map.get(ticker) {
            return Some(r.base_precision);
        }
        map.iter()
            .filter(|(base, r)| r.symbol == format!("{base}{ticker}"))
            .map(|(_, r)| r.quote_asset_precision)
            .min()
    }

    /// Символ MEXC пары монеты. None — пары нет. Ошибок нет.
    pub fn symbol_of(&self, ticker: &str) -> Option<String> {
        let map = self.map.read().unwrap_or_else(|p| p.into_inner());
        map.get(ticker).map(|r| r.symbol.clone())
    }
}
