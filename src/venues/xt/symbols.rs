//! Файл: venues/xt/symbols.rs
//! Что делает: правила торговли пар XT.com (шаги и пределы цены и количества, минимум суммы, точности, состояние,
//! торговля через API, ставка тейкера пары) — справочник в памяти с обновлением по запросу. При той же версии
//! справочника (version) XT список не присылает — прежний остаётся, время получения обновляется (решение 131).
//! Кто вызывает: order_rules.rs, trade.rs, pairs.rs, pairs_watch.rs, market.rs, book.rs.
//! От чего зависит: rest.rs, symbols_parse.rs (разбор), range.rs (пределы и шаг), ctx.rs, order_rules.rs (признак
//! «не примет наш ордер» — решение 254).
//! Настройки: venue.quote_asset, venue.symbols_refresh_s.
//!
//! Официально (https://doc.xt.com/#marketsymbol, `_market/2symbol.md`): GET /v4/public/symbol (10/s/ip), параметр
//! version — «when the request version number is consistent with the response content version, the list will not be
//! returned»; перечень состояний пары — publicModule.md «Symbol state»: ONLINE, OFFLINE, DELISTED.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::events::SpotPairState;

use super::ctx::Ctx;
pub use super::range::Range;
use super::rest;
use super::symbols_parse::parse_list;

/// Состояния пары из перечня publicModule.md по порядку; код — номер в перечне (ONLINE = 1). Это перечень
/// документации, не настройка.
const STATES: [&str; 3] = ["ONLINE", "OFFLINE", "DELISTED"];
/// Код состояния «пара торгуется» (ONLINE).
pub const STATE_ONLINE: i64 = 1;

/// Код состояния по имени (1-based номер в перечне; неизвестное — −1). Ошибок нет.
pub fn state_code(s: &str) -> i64 {
    STATES
        .iter()
        .position(|x| *x == s)
        .map_or(-1, |i| i as i64 + 1)
}

/// Имя состояния по коду (для журнала). Ошибок нет.
pub fn state_name(code: i64) -> &'static str {
    usize::try_from(code - 1)
        .ok()
        .and_then(|i| STATES.get(i).copied())
        .unwrap_or("неизвестное")
}

/// Правила одной пары.
#[derive(Debug, Clone)]
pub struct SymbolRules {
    /// Символ XT, например cake_usdt (так он приходит в справочнике).
    pub symbol: String,
    /// Тикер ядра (baseCurrency заглавными).
    pub ticker: String,
    pub price: Range,
    pub qty: Range,
    /// QUOTE_QTY.min — минимум суммы price × quantity у LIMIT.
    pub quote_min: Option<Decimal>,
    /// takerFeeRate пары (доля). None — XT не сообщил: котировки нет.
    pub taker_fee: Option<Decimal>,
    /// Код состояния (state_code) и имя как в ответе.
    pub status: i64,
    pub state_text: String,
    pub trading_enabled: bool,
    pub openapi_enabled: bool,
    /// Можно купить / продать через API: tradingEnabled и openapiEnabled.
    pub api_buy: bool,
    pub api_sell: bool,
    /// orderTypes и timeInForces пары; None — поля нет.
    pub order_types: Option<Vec<String>>,
    pub time_in_forces: Option<Vec<String>>,
}

impl SymbolRules {
    /// Пара торгуется через API в указанную сторону. Ошибок нет.
    pub fn tradable(&self, buy: bool) -> bool {
        self.status == STATE_ONLINE && if buy { self.api_buy } else { self.api_sell }
    }
}

/// Справочник и его версия, когда получен (для срока — Instant, для ядра — время UTC).
#[derive(Default)]
struct State {
    map: Arc<HashMap<String, SymbolRules>>,
    version: Option<String>,
    loaded: Option<(Instant, DateTime<Utc>)>,
}

/// Справочник пар (ключ — тикер ядра). Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Symbols {
    st: Arc<RwLock<State>>,
}

/// Пустой список при той же версии, что у прежнего справочника, — «без изменений». Принимает: число пар ответа,
/// версию ответа, прежнюю версию. Ошибок нет.
pub fn unchanged_empty(n: usize, got: &Option<String>, had: &Option<String>) -> bool {
    n == 0 && got.is_some() && got == had
}

impl Symbols {
    /// Загружает пары с котируемой монетой venue.quote_asset (с версией прежнего ответа). Возвращает: число пар.
    /// Ошибки: связь, ответ XT, настройки.
    pub async fn refresh(&self, ctx: &Ctx) -> GatewayResult<usize> {
        let quote = ctx.text("venue.quote_asset")?;
        let version = self
            .st
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .version
            .clone();
        let p = version.map(|v| vec![("version", v)]).unwrap_or_default();
        let data = rest::public_heavy(ctx, "/v4/public/symbol", p).await?;
        let c = ctx.clone();
        let parsed = tokio::task::spawn_blocking(move || parse_list(&data, &quote))
            .await
            .map_err(|e| {
                c.tag(GatewayError::new(
                    ErrorCode::Internal,
                    None,
                    format!("разбор /v4/public/symbol прерван: {e}"),
                ))
            })??;
        let mut st = self.st.write().unwrap_or_else(|p| p.into_inner());
        // 06.11.2026 (живой бот, 21:05:55): ответ с той же версией дал «нет пары ONLINE» у всех 282 монет — XT прислал
        // пустой список symbols, а не убрал поле. Документация («the list will not be returned») не говорит, как именно.
        // Пустой список при той же версии, что у прежнего справочника, — «без изменений», как отсутствие поля.
        let map = parsed
            .map
            .filter(|m| !unchanged_empty(m.len(), &parsed.version, &st.version));
        match map {
            Some(map) => st.map = Arc::new(map),
            // Список не прислан (или пуст при той же версии): прежний справочник верен. Его ещё не было — ошибка.
            None if st.loaded.is_some() => {}
            None => {
                return Err(super::errors::bad(
                    "справочник пар без списка symbols при первом запросе",
                    "",
                ));
            }
        }
        st.version = parsed.version;
        st.loaded = Some((Instant::now(), Utc::now()));
        Ok(st.map.len())
    }

    /// Ставит разобранный справочник (тесты на записанных ответах). Ошибок нет.
    pub fn load(&self, map: HashMap<String, SymbolRules>, at: DateTime<Utc>) {
        let mut st = self.st.write().unwrap_or_else(|p| p.into_inner());
        st.map = Arc::new(map);
        st.loaded = Some((Instant::now(), at));
    }

    /// Когда справочник последний раз успешно получен. Ошибок нет.
    pub fn loaded_at(&self) -> Option<DateTime<Utc>> {
        self.st
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .loaded
            .map(|(_, at)| at)
    }

    /// Перечитывает справочник, если он не загружен или старше venue.symbols_refresh_s. Ошибки: как у refresh.
    pub async fn ensure_fresh(&self, ctx: &Ctx) -> GatewayResult<()> {
        let max_age = Duration::from_secs(ctx.int("venue.symbols_refresh_s")?);
        let loaded = self.st.read().unwrap_or_else(|p| p.into_inner()).loaded;
        if loaded.is_none_or(|(at, _)| at.elapsed() >= max_age) {
            self.refresh(ctx).await?;
        }
        Ok(())
    }

    /// Справочник целиком (указатель, без копии). Ошибок нет.
    fn all(&self) -> Arc<HashMap<String, SymbolRules>> {
        self.st
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .map
            .clone()
    }

    /// Правила пары по тикеру. Ошибка BadResponse — такой пары у XT нет.
    pub fn get(&self, ctx: &Ctx, ticker: &str) -> GatewayResult<SymbolRules> {
        self.all().get(ticker).cloned().ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::BadResponse,
                None,
                format!("Пары {ticker} нет в справочнике XT"),
            ))
        })
    }

    /// Ставка тейкера пары (takerFeeRate). None — пары нет или ставки нет. Ошибок нет.
    pub fn taker_fee(&self, ticker: &str) -> Option<Decimal> {
        self.all().get(ticker).and_then(|r| r.taker_fee)
    }

    /// Состояние пары монеты для ядра (MarketEvent::SpotPairs). Принимает: тикер, timeInForce из настройки
    /// venue.order_time_in_force (None — не задан) — для признака «XT не примет наш ордер» (решение 254,
    /// order_rules::order_block). None — пары нет. Ошибок нет.
    pub fn state(&self, ticker: &str, tif: Option<&str>) -> Option<SpotPairState> {
        self.all().get(ticker).map(|r| SpotPairState {
            ticker: ticker.to_string(),
            status: r.status,
            online: r.status == STATE_ONLINE,
            api_buy: r.api_buy,
            api_sell: r.api_sell,
            step_size: r.qty.step().unwrap_or(Decimal::ZERO),
            min_notional: r.quote_min,
            max_notional: None,
            min_qty: r.qty.min,
            max_qty: r.qty.max,
            order_block: super::order_rules::order_block(r, tif),
        })
    }

    /// Символ XT пары монеты. None — пары нет. Ошибок нет.
    pub fn symbol_of(&self, ticker: &str) -> Option<String> {
        self.all().get(ticker).map(|r| r.symbol.clone())
    }
}

#[cfg(test)]
mod same_version_tests {
    use super::unchanged_empty;

    #[test]
    fn empty_list_same_version_is_unchanged() {
        let v = Some("123".to_string());
        assert!(unchanged_empty(0, &v, &v));
        assert!(
            !unchanged_empty(0, &Some("124".into()), &v),
            "новая версия и пусто — честно пусто"
        );
        assert!(!unchanged_empty(5, &v, &v));
        assert!(!unchanged_empty(0, &None, &None));
    }
}
