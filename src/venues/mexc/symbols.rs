//! Файл: venues/mexc/symbols.rs
//! Что делает: правила торговли пар MEXC (шаг цены, шаг количества, минимальное количество и сумма, статус, разрешены
//! ли покупка и продажа через API нашему ключу) — справочник в памяти с обновлением по запросу.
//! Кто вызывает: trade.rs (округление и проверки перед ордером), pairs.rs (состояние пар для ядра), pairs_watch.rs.
//! От чего зависит: rest.rs, num.rs, ctx.rs.
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

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use reqwest::Method;
use rust_decimal::Decimal;
use serde_json::Value;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::events::SpotPairState;

use super::ctx::Ctx;
use super::errors::Op;
use super::{num, rest};

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
}

impl SymbolRules {
    /// Пара торгуется через API в указанную сторону.
    pub fn tradable(&self, buy: bool) -> bool {
        self.status == STATUS_ONLINE && if buy { self.api_buy } else { self.api_sell }
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

    /// Состояние пары монеты для ядра (MarketEvent::SpotPairs). None — пары нет. Ошибок нет.
    pub fn state(&self, ticker: &str) -> Option<SpotPairState> {
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
        })
    }

    /// Точность вывода монеты (решение 107): у монеты пары — baseAssetPrecision; у котируемой монеты (USDT) —
    /// quoteAssetPrecision любой её пары (наименьшая — безопасно). None — неизвестно. Ошибок нет.
    pub fn precision_of(&self, ticker: &str) -> Option<u32> {
        let map = self.map.read().unwrap_or_else(|p| p.into_inner());
        if let Some(r) = map.get(ticker) {
            return Some(r.base_precision);
        }
        map.values()
            .filter(|r| r.symbol.ends_with(ticker))
            .map(|r| r.quote_asset_precision)
            .min()
    }

    /// Символ MEXC пары монеты. None — пары нет. Ошибок нет.
    pub fn symbol_of(&self, ticker: &str) -> Option<String> {
        let map = self.map.read().unwrap_or_else(|p| p.into_inner());
        map.get(ticker).map(|r| r.symbol.clone())
    }
}

/// Список пар, разрешённых ключу (selfSymbols): массив символов в data (ответ в обёртке {code: 200, data}).
/// Ошибка BadResponse — не массив строк.
pub fn self_symbols(data: &Value) -> GatewayResult<HashSet<String>> {
    let arr = data
        .as_array()
        .ok_or_else(|| super::errors::bad("selfSymbols: data не массив", &data.to_string()))?;
    arr.iter()
        .map(|v| {
            v.as_str()
                .map(str::to_string)
                .ok_or_else(|| super::errors::bad("selfSymbols: не строка", &v.to_string()))
        })
        .collect()
}

/// Разбор exchangeInfo: пары с котируемой монетой quote (ключ — тикер монеты). allowed — selfSymbols ключа.
/// Ошибка BadResponse — неверный формат.
pub fn parse_list(
    data: &Value,
    quote: &str,
    allowed: &HashSet<String>,
) -> GatewayResult<HashMap<String, SymbolRules>> {
    let mut map = HashMap::new();
    for s in num::arr(data, "symbols")? {
        if num::text_opt(s, "quoteAsset").as_deref() != Some(quote) {
            continue;
        }
        let ticker = num::text(s, "baseAsset")?;
        map.insert(ticker, parse(s, allowed)?);
    }
    Ok(map)
}

/// Разбор одной пары. Ошибка BadResponse — нет обязательных полей.
fn parse(s: &Value, allowed: &HashSet<String>) -> GatewayResult<SymbolRules> {
    let symbol = num::text(s, "symbol")?;
    let status = match num::text(s, "status")?.as_str() {
        "1" | "ENABLED" => STATUS_ONLINE,
        "2" => 2,
        "3" => 3,
        _ => -1,
    };
    let spot_ok = matches!(s.get("isSpotTradingAllowed"), Some(Value::Bool(true)));
    let own = allowed.contains(&symbol);
    // tradeSideType: 1 все, 2 только покупка, 3 только продажа, 4 закрытие (нет поля — без ограничения стороны).
    let side = num::text_opt(s, "tradeSideType").unwrap_or_else(|| "1".into());
    let (buy_side, sell_side) = match side.as_str() {
        "2" => (true, false),
        "3" => (false, true),
        "4" => (false, false),
        _ => (true, true),
    };
    Ok(SymbolRules {
        tick_size: pow10_neg(num::int(s, "quotePrecision")?),
        step_size: pow10_neg(num::int(s, "baseAssetPrecision")?),
        base_precision: num::int(s, "baseAssetPrecision")?.clamp(0, 28) as u32,
        quote_asset_precision: num::int(s, "quoteAssetPrecision")?.clamp(0, 28) as u32,
        min_notional: num::dec_opt(s, "quoteAmountPrecision")?,
        max_notional: num::dec_opt(s, "maxQuoteAmount")?,
        min_qty: num::dec_opt(s, "baseSizePrecision")?,
        status,
        api_buy: spot_ok && own && buy_side,
        api_sell: spot_ok && own && sell_side,
        symbol,
    })
}

/// 10^−n (n знаков после запятой): 2 → 0.01, 0 → 1. Отрицательное n — 1. Ошибок нет.
fn pow10_neg(n: i64) -> Decimal {
    let n = n.clamp(0, 28) as u32;
    Decimal::new(1, n)
}
