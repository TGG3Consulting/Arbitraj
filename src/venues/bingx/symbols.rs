//! Файл: venues/bingx/symbols.rs
//! Что делает: правила торговли пар BingX (шаг цены, шаг количества, минимальная сумма, статус,
//! разрешены ли покупка и продажа через API) — справочник в памяти с обновлением по запросу.
//! Кто вызывает: trade.rs (округление и проверки перед ордером), market.rs (список пар для подписки).
//! От чего зависит: rest.rs, num.rs, ctx.rs.
//! Настройки: venue.quote_asset, venue.symbols_refresh_s (как часто перечитывать правила пар).
//!
//! Официальная документация: «Spot trading symbols» GET /openApi/spot/v1/common/symbols
//! https://bingx-api.github.io/docs-v3/#/en/Spot/Market%20Data/Spot%20trading%20symbols
//! status: 0 offline, 1 online, 5 pre-open, 10 accessed, 25 suspended, 29 pre-delisted, 30 delisted.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use rust_decimal::Decimal;
use serde_json::Value;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::{num, rest};

/// Статус «пара торгуется» — по официальной документации.
pub const STATUS_ONLINE: i64 = 1;

/// Правила одной пары.
#[derive(Debug, Clone)]
pub struct SymbolRules {
    /// Символ BingX, например CAKE-USDT.
    pub symbol: String,
    pub tick_size: Decimal,
    pub step_size: Decimal,
    pub min_notional: Option<Decimal>,
    pub max_notional: Option<Decimal>,
    /// Статус по документации.
    pub status: i64,
    pub api_buy: bool,
    pub api_sell: bool,
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
}

impl Symbols {
    /// Загружает все пары с котируемой монетой venue.quote_asset.
    /// Возвращает: число пар. Ошибки: связь, ответ BingX, настройки.
    pub async fn refresh(&self, ctx: &Ctx) -> GatewayResult<usize> {
        let quote = ctx.text("venue.quote_asset")?;
        let data = rest::public(ctx, "/openApi/spot/v1/common/symbols", vec![]).await?;
        let map = parse_list(&data, &quote)?;
        let n = map.len();
        *self.map.write().unwrap_or_else(|p| p.into_inner()) = map;
        *self.loaded.write().unwrap_or_else(|p| p.into_inner()) = Some(Instant::now());
        Ok(n)
    }

    /// Перечитывает справочник, если он не загружен или старше venue.symbols_refresh_s.
    /// Ошибки: настройки, связь, ответ BingX.
    pub async fn ensure_fresh(&self, ctx: &Ctx) -> GatewayResult<()> {
        let max_age = Duration::from_secs(ctx.int("venue.symbols_refresh_s")?);
        let loaded = *self.loaded.read().unwrap_or_else(|p| p.into_inner());
        if loaded.is_none_or(|at| at.elapsed() >= max_age) {
            self.refresh(ctx).await?;
        }
        Ok(())
    }

    /// Правила пары по тикеру. Ошибка BadResponse — такой пары у BingX нет (или справочник не загружен).
    pub fn get(&self, ctx: &Ctx, ticker: &str) -> GatewayResult<SymbolRules> {
        let map = self.map.read().unwrap_or_else(|p| p.into_inner());
        map.get(ticker).cloned().ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::BadResponse,
                None,
                format!("Пары {ticker} нет в справочнике BingX"),
            ))
        })
    }

    /// Все тикеры, которые сейчас торгуются (статус online).
    pub fn online(&self) -> Vec<String> {
        let map = self.map.read().unwrap_or_else(|p| p.into_inner());
        map.iter()
            .filter(|(_, r)| r.status == STATUS_ONLINE)
            .map(|(t, _)| t.clone())
            .collect()
    }
}

/// Разбор ответа «Spot trading symbols»: пары с котируемой монетой quote (ключ — тикер монеты).
/// Ошибка BadResponse — неверный формат.
pub fn parse_list(data: &Value, quote: &str) -> GatewayResult<HashMap<String, SymbolRules>> {
    let suffix = format!("-{quote}");
    let mut map = HashMap::new();
    for s in num::arr(data, "symbols")? {
        let symbol = num::text(s, "symbol")?;
        if let Some(ticker) = symbol.strip_suffix(&suffix) {
            map.insert(ticker.to_string(), parse(s, symbol.clone())?);
        }
    }
    Ok(map)
}

/// Разбор одной пары. Ошибка BadResponse — нет обязательных полей.
fn parse(s: &Value, symbol: String) -> GatewayResult<SymbolRules> {
    Ok(SymbolRules {
        symbol,
        tick_size: num::dec(s, "tickSize")?,
        step_size: num::dec(s, "stepSize")?,
        min_notional: num::dec_opt(s, "minNotional")?,
        max_notional: num::dec_opt(s, "maxNotional")?,
        status: num::int(s, "status")?,
        // Поля apiStateBuy/apiStateSell описаны в документации; если BingX их не прислал — считаем «нельзя».
        api_buy: s
            .get("apiStateBuy")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        api_sell: s
            .get("apiStateSell")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}
