//! Файл: venues/bingx/symbols_qty.rs
//! Что делает: решение 293 — необязательная настройка venue.ignore_deprecated_qty [bingx]: «не соблюдать устаревшие
//! minQty / maxQty справочника пар». Области: площадка и монета (значение монеты сильнее — можно включить сначала у
//! одной монеты, например SHOPINX). Ключ монеты — как у всех настроек области «монета» (решение 211,
//! settings/scope.rs coin_id): адрес контракта BSC строчными; адрес берётся из монеты запроса ядра (Asset.bsc_contract:
//! ордер — TradeRequest.asset, состояние пары — монеты подписки). Адреса у монеты нет — значение монеты не
//! применяется, действует значение площадки. Старое значение по тикеру («coin:SHOPINX») читается только после
//! переноса на адрес (core/coin_scope_migrate.rs): тикеры-синонимы адреса settings/store.rs resolve применяет только
//! для coin.blocked = да, у этой настройки старое значение по тикеру без переноса не действует.
//! «Да» у пары — её правила без пределов количества (min_qty, max_qty = None); остаются minNotional / maxNotional,
//! шаги, статус, apiStateBuy / apiStateSell. Справочник хранится как ответил BingX; пределы снимаются при чтении:
//! Symbols::get_for — правила ордера (order_rules.rs: лимитный ордер, рыночная продажа market_sell.rs, проверка без
//! исполнения check_order), Symbols::state_for — состояние пары для ядра (pairs.rs → SpotPairState: core/signal_probe.rs
//! floor_qty, core/costs_quote.rs order_limits, core/dust.rs). Ядро о настройке не знает.
//! Не задана или «нет» — 1 в 1 как раньше (решение 66: соблюдаются оба предела). Неверно сохранённое значение у монеты —
//! у неё как «не задано» (действует значение площадки), у площадки — как «не задано»; Warn в журнал venue.symbols один
//! раз за процесс на область; справочник обновляется как обычно.
//! Когда действует: правила ордера — сразу (читают настройку на каждом ордере); ядро — со следующего снимка пар
//! (venue.symbols_refresh_s, pairs_watch.rs). Окно рассогласования — до одного периода: при «да → нет» продажа уже
//! начатой сделки может получить локальный отказ правил и алерт — выключать, когда нет открытых сделок по BingX.
//! Кто вызывает: symbols.rs (Symbols::refresh → take), order_rules.rs, market_sell.rs, pairs.rs, тесты.
//! От чего зависит: symbols.rs (parse_list, SymbolRules, Symbols), ctx.rs, settings (store, scope).
//! Настройки: venue.ignore_deprecated_qty, venue.quote_asset.
//!
//! Официальная документация: «Spot trading symbols» GET /openApi/spot/v1/common/symbols
//! https://bingx-api.github.io/docs-v3/#/en/Spot/Market%20Data/Spot%20trading%20symbols — minQty: «Deprecated; can be
//! calculated using minQty=minNotional/price», maxQty: «Deprecated; can be calculated using maxQty=maxNotional/price».
//! Place order (docs-v3 Spot → Trade): «If you need to calculate the maximum and minimum order quantities for a currency
//! pair, you can use the formula: U (minNotional or maxNotional) / (Limit price or Market price)».
//! Противоречие источников: api-ai-skills skills/spot-market/api-reference.md (тоже официальный, BingX-API) — minQty
//! «Minimum order quantity (base asset)», без пометки об устаревании (как в шапке поля min_qty, symbols.rs).

use std::collections::HashSet;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::core::types::Asset;
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::gateway::events::SpotPairState;
use crate::settings::read::At;
use crate::settings::scope::{Scope, coin_id};
use crate::settings::store::Store;
use crate::storage::journal::Level;

use super::ctx::{Ctx, ID};
use super::symbols::{SymbolRules, Symbols, parse_list};

/// Имя настройки (каталог — settings/catalog/bingx.rs, docs/SETTINGS.md).
pub const SETTING: &str = "venue.ignore_deprecated_qty";

/// Области с неверно сохранённым значением, о которых уже сказано в журнале (один раз за процесс).
static WARNED: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// Что делает: «не соблюдать minQty / maxQty» для пары монеты. Значение монеты (ключ — coin_id по адресу) сильнее
/// значения площадки; адреса нет — только площадка. Неверно сохранённое значение — как «не задано» (у монеты —
/// дальше значение площадки). Принимает: хранилище настроек, монету. Возвращает: true — пределы не соблюдаются.
/// Ошибок нет.
pub fn ignored(store: &Store, asset: &Asset) -> bool {
    let venue_only = At {
        venue: Some(ID),
        coin: None,
    };
    let venue = || store.flag(SETTING, venue_only).unwrap_or(false);
    let Some(addr) = asset.bsc_contract.as_ref() else {
        return venue();
    };
    let coin = coin_id(&asset.ticker, Some(addr));
    let at = At {
        venue: Some(ID),
        coin: Some(&coin),
    };
    match store.flag(SETTING, at) {
        Ok(v) => v,
        Err(e) if e.code == ErrorCode::NotConfigured => false,
        // Неверное значение у монеты — как «не задано»: значение площадки (о неверном — warn_bad).
        Err(_) => venue(),
    }
}

impl Symbols {
    /// Что делает: правила пары монеты для ордера — без minQty / maxQty, если для неё venue.ignore_deprecated_qty = да.
    /// Принимает: окружение, монету (тикер и адрес). Возвращает: правила. Ошибка BadResponse — пары нет в справочнике.
    pub fn get_for(&self, ctx: &Ctx, asset: &Asset) -> GatewayResult<SymbolRules> {
        let mut r = self.get(ctx, &asset.ticker)?;
        if ignored(&ctx.settings, asset) {
            r.min_qty = None;
            r.max_qty = None;
        }
        Ok(r)
    }

    /// Что делает: состояние пары монеты для ядра — без minQty / maxQty по настройке (хранилище — из последнего take;
    /// справочник, поставленный без take, — как раньше). Принимает: монету. Возвращает: состояние или None — пары нет.
    /// Ошибок нет.
    pub fn state_for(&self, asset: &Asset) -> Option<SpotPairState> {
        let mut p = self.state(&asset.ticker)?;
        let store = self.qty_store.read().unwrap_or_else(|e| e.into_inner());
        if store.as_ref().is_some_and(|s| ignored(s, asset)) {
            p.min_qty = None;
            p.max_qty = None;
        }
        Some(p)
    }
}

/// Что делает: читает venue.quote_asset и разбирает ответ (take_quoted). Для тестов на записанном ответе.
/// Принимает: окружение, справочник пар, data ответа, время получения. Возвращает: число пар.
/// Ошибки: NotConfigured (venue.quote_asset), BadResponse (формат ответа).
pub fn take(ctx: &Ctx, symbols: &Symbols, data: &Value, at: DateTime<Utc>) -> GatewayResult<usize> {
    let quote = ctx.text("venue.quote_asset")?;
    take_quoted(ctx, symbols, data, &quote, at)
}

/// Что делает: разбирает ответ «Spot trading symbols» (поле data) по котируемой монете и ставит справочник в память с
/// временем получения; запоминает хранилище настроек для state_for. Вызывает Symbols::refresh (котируемую монету он
/// прочитал до запроса к сети) и take. Принимает: окружение, справочник пар, data ответа, котируемую монету, время.
/// Возвращает: число пар. Ошибки: BadResponse (формат ответа).
pub fn take_quoted(
    ctx: &Ctx,
    symbols: &Symbols,
    data: &Value,
    quote: &str,
    at: DateTime<Utc>,
) -> GatewayResult<usize> {
    let map = parse_list(data, quote)?;
    let n = map.len();
    symbols.load(map, at);
    *symbols.qty_store.write().unwrap_or_else(|e| e.into_inner()) = Some(ctx.settings.clone());
    Ok(n)
}

/// Что делает: неверно сохранённые значения настройки (не да/нет) — Warn в журнал venue.symbols, один раз за процесс
/// на область. Принимает: окружение. Возвращает: области, о которых сказано сейчас. Ошибок нет.
pub async fn warn_bad(ctx: &Ctx) -> Vec<String> {
    let bad: Vec<String> = ctx
        .settings
        .all()
        .into_iter()
        .filter(|(n, sc, v)| n == SETTING && !v.is_boolean() && in_scope(sc))
        .map(|(_, sc, _)| sc.to_key())
        .collect();
    let fresh: Vec<String> = {
        let mut w = WARNED.lock().unwrap_or_else(|e| e.into_inner());
        let set = w.get_or_insert_with(HashSet::new);
        bad.into_iter().filter(|k| set.insert(k.clone())).collect()
    };
    for k in &fresh {
        let m = format!("{SETTING} [{k}] хранится в неверном виде — считается «не задано»");
        let _ = ctx
            .journal
            .write(Level::Warn, "venue.symbols", Some(ID), m, None)
            .await;
    }
    fresh
}

/// Область значения, которое читает адаптер BingX: монета или площадка «bingx». Ошибок нет.
fn in_scope(sc: &Scope) -> bool {
    match sc {
        Scope::Coin(_) => true,
        Scope::Venue(v) => v == ID,
        _ => false,
    }
}
