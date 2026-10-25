//! Файл: venues/bingx/symbols.rs
//! Что делает: правила торговли пар BingX (шаг цены, шаг количества, минимальная сумма, статус,
//! разрешены ли покупка и продажа через API) — справочник в памяти с обновлением по запросу.
//! Кто вызывает: trade.rs (округление и проверки перед ордером), pairs.rs (состояние пар подписки для ядра),
//! pairs_watch.rs (обновление раз в venue.symbols_refresh_s).
//! От чего зависит: rest.rs, num.rs, ctx.rs.
//! Настройки: venue.quote_asset, venue.symbols_refresh_s (как часто перечитывать правила пар).
//!
//! Официальная документация: «Spot trading symbols» GET /openApi/spot/v1/common/symbols
//! https://bingx-api.github.io/docs-v3/#/en/Spot/Market%20Data/Spot%20trading%20symbols
//! status: 0 offline, 1 online, 5 pre-open, 10 accessed, 25 suspended, 29 pre-delisted, 30 delisted (docs-v3;
//! в api-ai-skills/spot-market — только «1 = active, 0 = inactive»). Торгуется только status 1; что значит 10
//! («accessed») — в документации не объяснено, вопрос Тиграну (PR bingx/symbols-filter): ядру уходит код как есть.
//! apiStateBuy / apiStateSell — можно ли купить / продать через API; в живом ответе 2026-10-24 — boolean у всех пар.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};

use rust_decimal::Decimal;
use serde_json::Value;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::events::SpotPairState;

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
    /// minQty / maxQty (решение 66): docs-v3 «Spot trading symbols» — «Deprecated; can be calculated using
    /// minQty=minNotional/price»; api-ai-skills skills/spot-market/api-reference.md — обычные поля «Minimum / Maximum
    /// order quantity». Живой ответ 2026-10-24: CAKE-USDT minQty 2.672 ≠ minNotional / price. Соблюдаются оба предела.
    pub min_qty: Option<Decimal>,
    pub max_qty: Option<Decimal>,
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
    /// Время успешного получения по часам (для ядра: свежесть справочника, core/pairs_fresh.rs).
    loaded_at: Arc<RwLock<Option<DateTime<Utc>>>>,
}

impl Symbols {
    /// Загружает все пары с котируемой монетой venue.quote_asset.
    /// Возвращает: число пар. Ошибки: связь, ответ BingX, настройки.
    pub async fn refresh(&self, ctx: &Ctx) -> GatewayResult<usize> {
        let quote = ctx.text("venue.quote_asset")?;
        let data = rest::public(ctx, "/openApi/spot/v1/common/symbols", vec![]).await?;
        let map = parse_list(&data, &quote)?;
        let n = map.len();
        self.load(map, Utc::now());
        Ok(n)
    }

    /// Ставит разобранный справочник (parse_list) и время получения at (ядру — SpotPairs.loaded_at). Вызывает
    /// refresh (at — сейчас); тесты — с записанным ответом и его временем. Ошибок нет.
    pub fn load(&self, map: HashMap<String, SymbolRules>, at: DateTime<Utc>) {
        *self.map.write().unwrap_or_else(|p| p.into_inner()) = map;
        *self.loaded.write().unwrap_or_else(|p| p.into_inner()) = Some(Instant::now());
        *self.loaded_at.write().unwrap_or_else(|p| p.into_inner()) = Some(at);
    }

    /// Когда справочник последний раз успешно получен. None — ещё не получен. Ошибок нет.
    pub fn loaded_at(&self) -> Option<DateTime<Utc>> {
        *self.loaded_at.read().unwrap_or_else(|p| p.into_inner())
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

    /// Состояние пары монеты для ядра (MarketEvent::SpotPairs). None — пары нет в справочнике. Ошибок нет.
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
            max_qty: r.max_qty,
        })
    }

    /// Символ BingX пары монеты (например CAKE-USDT). None — пары нет в справочнике. Ошибок нет.
    pub fn symbol_of(&self, ticker: &str) -> Option<String> {
        let map = self.map.read().unwrap_or_else(|p| p.into_inner());
        map.get(ticker).map(|r| r.symbol.clone())
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
        min_qty: num::dec_opt(s, "minQty")?,
        max_qty: num::dec_opt(s, "maxQty")?,
        status: num::int(s, "status")?,
        api_buy: api_flag(s, "apiStateBuy")?,
        api_sell: api_flag(s, "apiStateSell")?,
    })
}

/// Флаг apiStateBuy / apiStateSell — boolean (docs-v3 «Spot trading symbols»; в живом ответе 2026-10-24 — boolean у
/// всех 2293 пар). Нет поля или другой тип — ответ не понят: BadResponse с куском ответа, справочник целиком не
/// обновляется (действует прежний, ядро видит его возраст — core/pairs_fresh.rs). «Нельзя» по догадке не ставится.
/// Принимает: пару из ответа, имя поля. Ошибки: BadResponse.
pub fn api_flag(s: &Value, field: &str) -> GatewayResult<bool> {
    match s.get(field) {
        Some(Value::Bool(b)) => Ok(*b),
        other => {
            let got = other.map_or("нет поля".to_string(), |v| format!("{v}"));
            let sym = s.get("symbol").map_or(String::new(), |v| v.to_string());
            Err(super::errors::bad(
                &format!("common/symbols: {sym}: {field} не boolean ({got})"),
                &s.to_string(),
            ))
        }
    }
}
