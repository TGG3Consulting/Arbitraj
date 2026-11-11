//! Файл: venues/gate/symbols.rs
//! Что делает: правила торговли пар Gate (шаг цены и количества, минимумы и максимумы количества и суммы, статус,
//! можно ли покупать и продавать через API нашим ключом) — справочник в памяти с обновлением по запросу.
//! Кто вызывает: order_rules.rs (правила ордера), pairs.rs (состояние пар для ядра), pairs_watch.rs, wallet.rs (точность).
//! От чего зависит: rest.rs, symbols_parse.rs (разбор), ctx.rs.
//! Настройки: venue.quote_asset, venue.symbols_refresh_s.
//!
//! Официальная документация: GET /spot/currency_pairs (публичный) и GET /account/detail (подписанный, белый список пар
//! ключа) — https://www.gate.com/docs/developers/apiv4/en/ , поля — в шапке symbols_parse.rs.
//! Коды статуса для ядра (поле status события SpotPairs — «как отдаёт биржа»): у Gate статус — строка trade_status;
//! код — её номер в перечне документации: 1 tradable, 2 buyable, 3 sellable, 4 untradable, −1 — иное или нет поля.

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
pub use super::symbols_parse::{parse_list, whitelist};

/// «tradable: can be bought and sold».
pub const TRADABLE: i64 = 1;
/// «buyable: can be bought».
pub const BUYABLE: i64 = 2;
/// «sellable: can be sold».
pub const SELLABLE: i64 = 3;
/// «untradable: cannot be traded».
pub const UNTRADABLE: i64 = 4;

/// Код статуса по строке trade_status (см. шапку). Ошибок нет.
pub fn status_code(s: Option<&str>) -> i64 {
    match s {
        Some("tradable") => TRADABLE,
        Some("buyable") => BUYABLE,
        Some("sellable") => SELLABLE,
        Some("untradable") => UNTRADABLE,
        _ => -1,
    }
}

/// Правила одной пары.
#[derive(Debug, Clone)]
pub struct SymbolRules {
    /// Символ Gate, например CAKE_USDT.
    pub symbol: String,
    pub tick_size: Decimal,
    pub step_size: Decimal,
    pub min_qty: Option<Decimal>,
    pub max_qty: Option<Decimal>,
    pub min_notional: Option<Decimal>,
    pub max_notional: Option<Decimal>,
    /// Код статуса (см. шапку).
    pub status: i64,
    /// Можно купить / продать через API: trade_status, пара в белом списке ключа (если он задан), buy_start/sell_start.
    pub api_buy: bool,
    pub api_sell: bool,
    /// Знаков после запятой у количества (amount_precision).
    pub base_precision: u32,
}

impl SymbolRules {
    /// Пара торгуется по статусу хотя бы в одну сторону. Ошибок нет.
    pub fn online(&self) -> bool {
        matches!(self.status, TRADABLE | BUYABLE | SELLABLE)
    }

    /// Пара торгуется через API в указанную сторону. Ошибок нет.
    pub fn tradable(&self, buy: bool) -> bool {
        self.online() && if buy { self.api_buy } else { self.api_sell }
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
    /// Загружает все пары с котируемой монетой venue.quote_asset и белый список пар ключа.
    /// Возвращает: число пар. Ошибки: связь, ответ Gate, настройки, ключи (/account/detail — подписанный).
    pub async fn refresh(&self, ctx: &Ctx) -> GatewayResult<usize> {
        let quote = ctx.text("venue.quote_asset")?;
        let info = rest::public_heavy(ctx, "/spot/currency_pairs", vec![]).await?;
        let detail =
            rest::signed(ctx, Method::GET, "/account/detail", vec![], None, Op::Other).await?;
        let allowed = whitelist(&detail)?;
        let map = parse_list(&info, &quote, &allowed, Utc::now().timestamp())?;
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

    /// Правила пары по тикеру. Ошибка BadResponse — такой пары у Gate нет.
    pub fn get(&self, ctx: &Ctx, ticker: &str) -> GatewayResult<SymbolRules> {
        let map = self.map.read().unwrap_or_else(|p| p.into_inner());
        map.get(ticker).cloned().ok_or_else(|| {
            let m = format!("Пары {ticker} нет в справочнике Gate");
            ctx.tag(GatewayError::new(ErrorCode::BadResponse, None, m))
        })
    }

    /// Состояние пары монеты для ядра (MarketEvent::SpotPairs). None — пары нет. Ошибок нет.
    pub fn state(&self, ticker: &str) -> Option<SpotPairState> {
        let map = self.map.read().unwrap_or_else(|p| p.into_inner());
        map.get(ticker).map(|r| SpotPairState {
            ticker: ticker.to_string(),
            status: r.status,
            online: r.online(),
            api_buy: r.api_buy,
            api_sell: r.api_sell,
            step_size: r.step_size,
            min_notional: r.min_notional,
            max_notional: r.max_notional,
            min_qty: r.min_qty,
            max_qty: r.max_qty,
            order_block: None,
        })
    }

    /// Знаков после запятой у количества монеты пары (amount_precision) — запасная точность вывода, когда точность
    /// сети (decimal из /wallet/currency_chains) ещё не получена (wallet.rs). None — пары нет. Ошибок нет.
    pub fn precision_of(&self, ticker: &str) -> Option<u32> {
        let map = self.map.read().unwrap_or_else(|p| p.into_inner());
        map.get(ticker).map(|r| r.base_precision)
    }

    /// Символ Gate пары монеты. None — пары нет. Ошибок нет.
    pub fn symbol_of(&self, ticker: &str) -> Option<String> {
        let map = self.map.read().unwrap_or_else(|p| p.into_inner());
        map.get(ticker).map(|r| r.symbol.clone())
    }
}
