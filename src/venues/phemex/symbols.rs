//! Файл: venues/phemex/symbols.rs
//! Что делает: справочник Phemex в памяти — правила спот-пар (шаги цены и количества, пределы количества и суммы,
//! масштабы) и масштабы монет (valueScale: без них не прочитать ни баланс, ни депозит, ни комиссию) с обновлением по
//! запросу.
//! Кто вызывает: order_rules.rs, pairs.rs, pairs_watch.rs, market.rs, account.rs, deposits.rs, transfers.rs,
//! fund_move.rs, user_stream_conn.rs, mod.rs.
//! От чего зависит: rest.rs, symbols_parse.rs (разбор), ctx.rs.
//! Настройки: venue.quote_asset, venue.symbols_refresh_s, venue.pair_status_listed [phemex].
//!
//! Официально — Query product information GET /public/products (https://phemex-docs.github.io/#query-product-information);
//! поля — шапка symbols_parse.rs. Флагов «покупка/продажа через API» у Phemex нет: обе стороны = «пара торгуется».

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
pub use super::symbols_parse::{Parsed, parse_products};

/// Статус пары (свои коды для ядра): status равен venue.pair_status_listed — торгуется.
pub const STATUS_LISTED: i64 = 1;
/// status другой — не торгуется ботом.
pub const STATUS_OTHER: i64 = 0;
/// Поля status у пары нет — не торгуется ботом (проверить на живом ответе).
pub const STATUS_NO_FIELD: i64 = 9;

/// Правила одной спот-пары.
#[derive(Debug, Clone, PartialEq)]
pub struct SymbolRules {
    /// Символ Phemex, например sCAKEUSDT.
    pub symbol: String,
    /// Базовая и котируемая монеты.
    pub base: String,
    pub quote: String,
    /// Масштаб цены (…Ep), количества (valueScale базовой) и суммы (valueScale котируемой).
    pub price_scale: u32,
    pub base_scale: u32,
    pub quote_scale: u32,
    /// Шаг цены и количества в масштабированных целых (quoteTickSizeEv, baseTickSizeEv).
    pub tick_ep: i64,
    pub step_ev: i64,
    /// Те же шаги настоящими числами.
    pub tick_size: Decimal,
    pub step_size: Decimal,
    /// Пределы суммы (minOrderValueEv, maxOrderValueEv) и количества (maxBaseOrderSizeEv).
    pub min_notional: Option<Decimal>,
    pub max_notional: Option<Decimal>,
    pub max_qty: Option<Decimal>,
    /// STATUS_* и значение status как пришло.
    pub status: i64,
    pub status_text: String,
}

impl SymbolRules {
    /// Пара торгуется через API (в обе стороны одинаково). Ошибок нет.
    pub fn tradable(&self) -> bool {
        self.status == STATUS_LISTED
    }
}

/// Когда справочник получен: для срока (Instant) и для ядра (время).
type Loaded = Option<(Instant, DateTime<Utc>)>;

/// Справочник (ключ пар — тикер монеты). Дешёво клонируется; данные — за Arc (без копии всего справочника).
#[derive(Clone, Default)]
pub struct Symbols {
    map: Arc<RwLock<Arc<HashMap<String, SymbolRules>>>>,
    scales: Arc<RwLock<Arc<HashMap<String, u32>>>>,
    loaded: Arc<RwLock<Loaded>>,
    told: Arc<RwLock<String>>,
}

impl Symbols {
    /// Загружает пары с котируемой монетой venue.quote_asset и масштабы всех монет. Возвращает: число пар.
    /// Ошибки: связь, ответ Phemex, настройки. Пропущенные пары — в журнал (при изменении списка).
    pub async fn refresh(&self, ctx: &Ctx) -> GatewayResult<usize> {
        let quote = ctx.text("venue.quote_asset")?;
        let listed = ctx.text("venue.pair_status_listed")?;
        let data = rest::public(ctx, "/public/products", vec![], true).await?;
        let p = tokio::task::spawn_blocking(move || parse_products(&data, &quote, &listed))
            .await
            .map_err(|e| {
                ctx.tag(GatewayError::new(
                    ErrorCode::Internal,
                    None,
                    format!("разбор /public/products прерван: {e}"),
                ))
            })??;
        let n = p.pairs.len();
        let text = p.skipped.join("; ");
        self.load(p, Utc::now());
        let new = {
            let mut t = self.told.write().unwrap_or_else(|p| p.into_inner());
            let new = *t != text;
            *t = text.clone();
            new
        };
        if new && !text.is_empty() {
            let m = format!("products: пропущено (неразборчивые поля): {text}");
            ctx.log(Level::Warn, "venue.symbols", m).await;
        }
        Ok(n)
    }

    /// Ставит разобранный справочник и время получения. Ошибок нет.
    pub fn load(&self, p: Parsed, at: DateTime<Utc>) {
        *self.map.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(p.pairs);
        *self.scales.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(p.scales);
        *self.loaded.write().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), at));
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

    /// Все пары (указатель, без копии). Ошибок нет.
    pub fn all(&self) -> Arc<HashMap<String, SymbolRules>> {
        self.map.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Масштабы монет (указатель, без копии). Ошибок нет.
    pub fn scales(&self) -> Arc<HashMap<String, u32>> {
        self.scales
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Масштаб valueScale монеты. Ошибка BadResponse — монеты нет в справочнике (сумму не прочитать).
    pub fn scale(&self, ctx: &Ctx, currency: &str) -> GatewayResult<u32> {
        self.scales().get(currency).copied().ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::BadResponse,
                None,
                format!("Монеты {currency} нет в справочнике Phemex (valueScale неизвестен)"),
            ))
        })
    }

    /// Правила пары по тикеру. Ошибка BadResponse — такой пары у Phemex нет.
    pub fn get(&self, ctx: &Ctx, ticker: &str) -> GatewayResult<SymbolRules> {
        self.all().get(ticker).cloned().ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::BadResponse,
                None,
                format!("Пары {ticker} нет в справочнике Phemex"),
            ))
        })
    }

    /// Символ Phemex пары монеты. None — пары нет. Ошибок нет.
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
            min_qty: None,
            max_qty: r.max_qty,
        })
    }
}
