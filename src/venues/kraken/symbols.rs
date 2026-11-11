//! Файл: venues/kraken/symbols.rs
//! Что делает: правила торговли спот-пар Kraken (шаг цены и количества, знаки для чек-суммы стакана, минимумы
//! количества и суммы, торгуется ли пара) — справочник в памяти с обновлением по запросу.
//! Кто вызывает: order_rules.rs, pairs.rs, pairs_watch.rs, market.rs, fees.rs.
//! От чего зависит: rest.rs, symbols_parse.rs (разбор), ctx.rs.
//! Настройки: venue.quote_asset, venue.symbols_refresh_s [kraken].
//!
//! Официально — GET /0/public/AssetPairs (OpenAPI https://docs.kraken.com/openapi/spot-rest.yaml); поля — в шапке
//! symbols_parse.rs. Лимит публичных REST в документации не описан — справочник читается только по сроку.

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

/// Статус пары (свои коды для ядра): торгуется (status online).
pub const STATUS_ONLINE: i64 = 1;
/// cancel_only, reduce_only — не торгуется.
pub const STATUS_OFF: i64 = 0;
/// Прочее (post_only, limit_only и новые значения) — не торгуется ботом.
pub const STATUS_OTHER: i64 = 2;

/// Правила одной пары.
#[derive(Debug, Clone, PartialEq)]
pub struct SymbolRules {
    /// Имя пары Kraken для REST (ключ AssetPairs, например CAKEUSDT).
    pub symbol: String,
    /// Имя пары для WebSocket (wsname, например CAKE/USDT).
    pub wsname: String,
    pub tick_size: Decimal,
    /// 10^−lot_decimals.
    pub step_size: Decimal,
    /// Знаки количества (lot_decimals) — шаг ордера и формат чек-суммы стакана.
    pub lot_decimals: u32,
    /// Знаки цены (pair_decimals) — формат чек-суммы стакана.
    pub price_decimals: u32,
    /// ordermin — в базовой монете.
    pub min_qty: Option<Decimal>,
    /// costmin — в котируемой монете.
    pub min_cost: Option<Decimal>,
    /// STATUS_ONLINE / STATUS_OFF / STATUS_OTHER.
    pub status: i64,
    /// status как пришёл (для журнала и причины отказа).
    pub state: String,
}

impl SymbolRules {
    /// Пара торгуется через API (в обе стороны одинаково). Ошибок нет.
    pub fn tradable(&self) -> bool {
        self.status == STATUS_ONLINE
    }
}

/// Когда справочник получен: для срока (Instant) и для ядра (время).
type Loaded = Option<(Instant, DateTime<Utc>)>;

/// Справочник пар (ключ — тикер монеты, актив base). Дешёво клонируется; правила — за Arc (без копии справочника).
#[derive(Clone, Default)]
pub struct Symbols {
    map: Arc<RwLock<Arc<HashMap<String, SymbolRules>>>>,
    loaded: Arc<RwLock<Loaded>>,
    told: Arc<RwLock<String>>,
}

impl Symbols {
    /// Загружает все спот-пары с котируемой монетой venue.quote_asset. Возвращает: число пар.
    /// Ошибки: связь, ответ Kraken, настройки. Пары с неразборчивыми полями — в журнал (при изменении списка).
    pub async fn refresh(&self, ctx: &Ctx) -> GatewayResult<usize> {
        let quote = ctx.text("venue.quote_asset")?;
        let result = rest::public(ctx, "AssetPairs", vec![]).await?;
        let (map, skipped) = parse_list(&result, &quote)?;
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
                "AssetPairs: пропущено пар с неразборчивыми полями: {} — {text}",
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

    /// Правила пары по тикеру. Ошибка BadResponse — такой пары у Kraken нет.
    pub fn get(&self, ctx: &Ctx, ticker: &str) -> GatewayResult<SymbolRules> {
        self.all().get(ticker).cloned().ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::BadResponse,
                None,
                format!("Пары {ticker} нет в справочнике Kraken (AssetPairs)"),
            ))
        })
    }

    /// Имя пары для REST по тикеру. None — пары нет. Ошибок нет.
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
            min_notional: r.min_cost,
            max_notional: None,
            min_qty: r.min_qty,
            max_qty: None,
            order_block: None,
        })
    }
}
