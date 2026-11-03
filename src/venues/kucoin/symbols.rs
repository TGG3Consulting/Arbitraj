//! Файл: venues/kucoin/symbols.rs
//! Что делает: правила торговли пар KuCoin (шаг цены и количества, минимум и максимум количества, минимальная сумма,
//! торгуется ли пара, коэффициенты комиссии) — справочник в памяти с обновлением по запросу.
//! Кто вызывает: order_rules.rs (правила ордера), pairs.rs (состояние пар для ядра), pairs_watch.rs, fees.rs, book.rs.
//! От чего зависит: rest.rs, symbols_parse.rs (разбор), ctx.rs.
//! Настройки: venue.quote_asset, venue.symbols_refresh_s [kucoin].
//!
//! Официально — GET /api/v2/symbols (Public, вес 4) https://www.kucoin.com/docs-new/api-3470154; поля — в шапке
//! symbols_parse.rs. Отдельного списка «пары, разрешённые ключу» (как selfSymbols у MEXC) и запрета стороны у KuCoin в
//! документации нет: покупка и продажа через API = enableTrading. Пара на аукционе открытия (callauctionIsEnabled) —
//! не торгуется ботом: в фазах аукциона ордера не исполняются сразу («Allow add orders, don't allow cancel orders»),
//! а правило Тиграна — неисполненное снимается.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::events::SpotPairState;
use crate::storage::journal::Level;

use super::ctx::Ctx;
use super::reply::Pool;
use super::rest;
pub use super::symbols_parse::parse_list;

/// Статус пары (свои коды для ядра): торгуется.
pub const STATUS_ONLINE: i64 = 1;
/// enableTrading = false.
pub const STATUS_OFF: i64 = 0;
/// enableTrading = true, но идёт аукцион открытия (callauctionIsEnabled = true).
pub const STATUS_AUCTION: i64 = 2;

/// Правила одной пары.
#[derive(Debug, Clone, PartialEq)]
pub struct SymbolRules {
    /// Символ KuCoin, например CAKE-USDT.
    pub symbol: String,
    pub tick_size: Decimal,
    pub step_size: Decimal,
    pub min_qty: Option<Decimal>,
    pub max_qty: Option<Decimal>,
    pub min_notional: Option<Decimal>,
    /// STATUS_ONLINE / STATUS_OFF / STATUS_AUCTION.
    pub status: i64,
    /// Порог защиты цены (priceLimitRate) — правилами не проверяется (нет последней цены), ловит пробный ордер.
    pub price_limit_rate: Option<Decimal>,
    /// Коэффициенты комиссии пары (нет поля — 1).
    pub maker_coef: Decimal,
    pub taker_coef: Decimal,
    /// Монета комиссии по справочнику (факт — feeCurrency ордера).
    pub fee_currency: Option<String>,
}

impl SymbolRules {
    /// Пара торгуется через API (в обе стороны одинаково — у KuCoin запрета стороны нет). Ошибок нет.
    pub fn tradable(&self) -> bool {
        self.status == STATUS_ONLINE
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
    /// Ошибки: связь, ответ KuCoin, настройки. Пары с неразборчивыми полями — в журнал (при изменении списка).
    pub async fn refresh(&self, ctx: &Ctx) -> GatewayResult<usize> {
        let quote = ctx.text("venue.quote_asset")?;
        let data = rest::public(ctx, "/api/v2/symbols", vec![], Pool::Public).await?;
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

    /// Правила пары по тикеру. Ошибка BadResponse — такой пары у KuCoin нет.
    pub fn get(&self, ctx: &Ctx, ticker: &str) -> GatewayResult<SymbolRules> {
        self.all().get(ticker).cloned().ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::BadResponse,
                None,
                format!("Пары {ticker} нет в справочнике KuCoin"),
            ))
        })
    }

    /// Символ KuCoin пары монеты. None — пары нет. Ошибок нет.
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
            max_notional: None,
            min_qty: r.min_qty,
            max_qty: r.max_qty,
        })
    }
}
