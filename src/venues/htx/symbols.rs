//! Файл: venues/htx/symbols.rs
//! Что делает: правила торговли пар HTX (шаги цены и количества из точности pp / ap, минимум и максимум количества,
//! минимальная сумма, статус пары) — справочник в памяти с обновлением по запросу.
//! Кто вызывает: order_rules.rs (правила ордера), pairs.rs (состояние пар для ядра), pairs_watch.rs, fees.rs, mod.rs.
//! От чего зависит: rest.rs, symbols_parse.rs (разбор), ctx.rs.
//! Настройки: venue.quote_asset, venue.symbols_refresh_s [htx].
//!
//! Официально — Get Market Symbols Setting GET /v1/settings/common/market-symbols (без symbols — все пары)
//! https://huobiapi.github.io/docs/spot/v1/en/#get-market-symbols-setting; поля — в шапке symbols_parse.rs. Флагов
//! «покупка / продажа через API» по сторонам у HTX нет — торгуется только state online.

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

/// Статус пары (свои коды для ядра): offline.
pub const STATUS_OFFLINE: i64 = 0;
/// online — торгуется.
pub const STATUS_ONLINE: i64 = 1;
/// pre-online — ещё не открыта.
pub const STATUS_PRE_ONLINE: i64 = 2;
/// suspend — торговля приостановлена.
pub const STATUS_SUSPEND: i64 = 3;
/// fuse — «предохранитель» (торговля ограничена).
pub const STATUS_FUSE: i64 = 4;
/// Прочие значения state (unknown, not-online, transfer-board и вне перечня) — бот не торгует.
pub const STATUS_OTHER: i64 = 9;

/// Код статуса для ядра по state документации. Ошибок нет.
pub fn status_code(state: &str) -> i64 {
    match state {
        "online" => STATUS_ONLINE,
        "offline" => STATUS_OFFLINE,
        "pre-online" => STATUS_PRE_ONLINE,
        "suspend" => STATUS_SUSPEND,
        "fuse" => STATUS_FUSE,
        _ => STATUS_OTHER,
    }
}

/// Правила одной пары.
#[derive(Debug, Clone, PartialEq)]
pub struct SymbolRules {
    /// Символ HTX (строчными слитно), например cakeusdt.
    pub symbol: String,
    /// Базовая и котируемая монеты — тикеры ядра (заглавными).
    pub base: String,
    pub quote: String,
    /// Шаг цены = 10^−pp.
    pub tick_size: Decimal,
    /// Шаг количества = 10^−ap.
    pub step_size: Decimal,
    /// Минимум количества (строже из minoa и lominoa).
    pub min_qty: Option<Decimal>,
    /// Максимум количества (меньший из maxoa и lomaxoa).
    pub max_qty: Option<Decimal>,
    /// Максимум количества лимитной покупки и продажи (lomaxba, lomaxsa).
    pub max_buy: Option<Decimal>,
    pub max_sell: Option<Decimal>,
    /// Минимальная сумма ордера — minov.
    pub min_notional: Option<Decimal>,
    /// STATUS_*.
    pub status: i64,
    /// state как пришёл (для журнала).
    pub state: String,
    /// Поле at «trading by api interface» как пришло (значения не перечислены в документации — не проверяется).
    pub api: Option<String>,
}

impl SymbolRules {
    /// Пара торгуется (state online). Ошибок нет.
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
    /// Ошибки: связь, ответ HTX, настройки. Пары с неразборчивыми полями — в журнал (при изменении списка).
    pub async fn refresh(&self, ctx: &Ctx) -> GatewayResult<usize> {
        let quote = ctx.text("venue.quote_asset")?;
        // Все пары HTX — большой ответ: разбор JSON вне рабочих потоков (решение 131).
        let mut r = rest::req(
            reqwest::Method::GET,
            "/v1/settings/common/market-symbols",
            vec![],
            None,
            super::errors::Op::Other,
            false,
        );
        r.heavy = true;
        let data = rest::send(ctx, r).await?;
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

    /// Правила пары по тикеру. Ошибка BadResponse — такой пары у HTX нет.
    pub fn get(&self, ctx: &Ctx, ticker: &str) -> GatewayResult<SymbolRules> {
        self.all().get(ticker).cloned().ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::BadResponse,
                None,
                format!("Пары {ticker} нет в справочнике HTX"),
            ))
        })
    }

    /// Символ HTX пары монеты. None — пары нет. Ошибок нет.
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
            order_block: None,
        })
    }
}
