//! Файл: venues/bingx/symbols_qty.rs
//! Что делает: решение 293 — необязательная настройка venue.ignore_deprecated_qty [bingx]: «не соблюдать устаревшие
//! minQty / maxQty справочника пар». Области: площадка и монета (ревью 293: значение монеты сильнее — можно включить
//! сначала у одной монеты, например SHOPINX). «Да» у пары (своей монеты или площадки) — при разборе ответа «Spot
//! trading symbols» у этой пары убираются пределы количества (min_qty, max_qty = None); остаются minNotional /
//! maxNotional, шаги, статус, apiStateBuy / apiStateSell.
//! Одна точка: всё, что читает пределы пары, берёт их из справочника пар адаптера — правила ордера (order_rules.rs:
//! лимитный ордер, рыночная продажа market_sell.rs, проверка без исполнения check_order) и ядро через SpotPairState
//! (core/signal_probe.rs floor_qty, core/costs_quote.rs order_limits, core/dust.rs). Ядро о настройке не знает.
//! Не задана или «нет» — справочник 1 в 1 как раньше (решение 66: соблюдаются оба предела).
//! Новое значение действует со следующего чтения справочника (venue.symbols_refresh_s, pairs_watch.rs) — без перезапуска;
//! до этого адаптер и ядро могут расходиться не дольше одного периода (выключать, когда нет открытых сделок по BingX).
//! Кто вызывает: symbols.rs (Symbols::refresh), тесты (tests/bingx_minqty_293.rs).
//! От чего зависит: symbols.rs (parse_list, SymbolRules, Symbols::load), ctx.rs.
//! Настройки: venue.ignore_deprecated_qty, venue.quote_asset.
//!
//! Официальная документация: «Spot trading symbols» GET /openApi/spot/v1/common/symbols
//! https://bingx-api.github.io/docs-v3/#/en/Spot/Market%20Data/Spot%20trading%20symbols — minQty: «Deprecated; can be
//! calculated using minQty=minNotional/price», maxQty: «Deprecated; can be calculated using maxQty=maxNotional/price».
//! Place order (docs-v3 Spot → Trade): «If you need to calculate the maximum and minimum order quantities for a currency
//! pair, you can use the formula: U (minNotional or maxNotional) / (Limit price or Market price)».
//! Противоречие источников: api-ai-skills skills/spot-market/api-reference.md (тоже официальный, BingX-API) — minQty
//! «Minimum order quantity (base asset)», без пометки об устаревании (как в шапке поля min_qty, symbols.rs).

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::settings::read::At;

use super::ctx::{Ctx, ID};
use super::symbols::{SymbolRules, Symbols, parse_list};

/// Имя настройки (каталог — settings/catalog/bingx.rs, docs/SETTINGS.md).
pub const SETTING: &str = "venue.ignore_deprecated_qty";

/// Что делает: читает venue.ignore_deprecated_qty для пары монеты: значение монеты (область «монета», ревью 293) сильнее
/// значения площадки. Принимает: окружение адаптера, тикер монеты. Возвращает: true — пределы количества у этой пары
/// не соблюдаются; false — не задана нигде или «нет» (как раньше).
/// Ошибки: значение хранится в неверном виде (справочник не обновляется — действует прежний, ядро видит его возраст).
pub fn ignored(ctx: &Ctx, ticker: &str) -> GatewayResult<bool> {
    let at = At {
        venue: Some(ID),
        coin: Some(ticker),
    };
    match ctx.settings.flag(SETTING, at) {
        Ok(v) => Ok(v),
        Err(e) if e.code == ErrorCode::NotConfigured => Ok(false),
        Err(e) => Err(ctx.tag(e)),
    }
}

/// Что делает: убирает пределы количества (minQty / maxQty) у пар, где venue.ignore_deprecated_qty = да (у монеты
/// или у площадки). Принимает: окружение, разобранный справочник (parse_list). Возвращает: ничего.
/// Ошибки: неверное значение настройки (справочник тогда не ставится целиком).
pub fn strip(ctx: &Ctx, map: &mut HashMap<String, SymbolRules>) -> GatewayResult<()> {
    for (ticker, r) in map.iter_mut() {
        if ignored(ctx, ticker)? {
            r.min_qty = None;
            r.max_qty = None;
        }
    }
    Ok(())
}

/// Что делает: разбирает ответ «Spot trading symbols» (поле data), по настройке убирает minQty / maxQty и ставит
/// справочник в память с временем получения. Тот же путь у Symbols::refresh и у тестов на записанном ответе.
/// Принимает: окружение, справочник пар, data ответа, время получения. Возвращает: число пар.
/// Ошибки: NotConfigured (venue.quote_asset), неверное значение настройки, BadResponse (формат ответа).
pub fn take(ctx: &Ctx, symbols: &Symbols, data: &Value, at: DateTime<Utc>) -> GatewayResult<usize> {
    let quote = ctx.text("venue.quote_asset")?;
    let mut map = parse_list(data, &quote)?;
    strip(ctx, &mut map)?;
    let n = map.len();
    symbols.load(map, at);
    Ok(n)
}
