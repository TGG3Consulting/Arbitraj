//! Файл: venues/bingx/symbols_qty.rs
//! Что делает: решение 293 — необязательная настройка venue.ignore_deprecated_qty [bingx]: «не соблюдать устаревшие
//! minQty / maxQty справочника пар». Включена — при разборе ответа «Spot trading symbols» у всех пар убираются пределы
//! количества (min_qty, max_qty = None); остаются minNotional / maxNotional, шаги, статус, apiStateBuy / apiStateSell.
//! Одна точка: всё, что читает пределы пары, берёт их из справочника пар адаптера — правила ордера (order_rules.rs:
//! лимитный ордер, рыночная продажа market_sell.rs, проверка без исполнения check_order) и ядро через SpotPairState
//! (core/signal_probe.rs floor_qty, core/costs_quote.rs order_limits, core/dust.rs). Ядро о настройке не знает.
//! Не задана или «нет» — справочник 1 в 1 как раньше (решение 66: соблюдаются оба предела).
//! Новое значение действует со следующего чтения справочника (venue.symbols_refresh_s, pairs_watch.rs) — без перезапуска.
//! Кто вызывает: symbols.rs (Symbols::refresh), тесты (tests/bingx_minqty_293.rs).
//! От чего зависит: symbols.rs (parse_list, SymbolRules, Symbols::load), ctx.rs.
//! Настройки: venue.ignore_deprecated_qty, venue.quote_asset.
//!
//! Официальная документация: «Spot trading symbols» GET /openApi/spot/v1/common/symbols
//! https://bingx-api.github.io/docs-v3/#/en/Spot/Market%20Data/Spot%20trading%20symbols — minQty: «Deprecated; can be
//! calculated using minQty=minNotional/price», maxQty: «Deprecated; can be calculated using maxQty=maxNotional/price».
//! Place order (docs-v3 Spot → Trade): «If you need to calculate the maximum and minimum order quantities for a currency
//! pair, you can use the formula: U (minNotional or maxNotional) / (Limit price or Market price)».

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::ctx::Ctx;
use super::symbols::{SymbolRules, Symbols, parse_list};

/// Имя настройки (каталог — settings/catalog/bingx.rs, docs/SETTINGS.md).
pub const SETTING: &str = "venue.ignore_deprecated_qty";

/// Что делает: читает venue.ignore_deprecated_qty площадки. Принимает: окружение адаптера.
/// Возвращает: true — пределы количества не соблюдаются; false — не задана или «нет» (как раньше).
/// Ошибки: значение хранится в неверном виде (справочник не обновляется — действует прежний, ядро видит его возраст).
pub fn ignored(ctx: &Ctx) -> GatewayResult<bool> {
    match ctx.flag(SETTING) {
        Ok(v) => Ok(v),
        Err(e) if e.code == ErrorCode::NotConfigured => Ok(false),
        Err(e) => Err(e),
    }
}

/// Что делает: убирает пределы количества (minQty / maxQty) у всех пар разобранного справочника.
/// Принимает: справочник (parse_list). Возвращает: ничего. Ошибок нет.
pub fn strip(map: &mut HashMap<String, SymbolRules>) {
    for r in map.values_mut() {
        r.min_qty = None;
        r.max_qty = None;
    }
}

/// Что делает: разбирает ответ «Spot trading symbols» (поле data), по настройке убирает minQty / maxQty и ставит
/// справочник в память с временем получения. Тот же путь у Symbols::refresh и у тестов на записанном ответе.
/// Принимает: окружение, справочник пар, data ответа, время получения. Возвращает: число пар.
/// Ошибки: NotConfigured (venue.quote_asset), неверное значение настройки, BadResponse (формат ответа).
pub fn take(ctx: &Ctx, symbols: &Symbols, data: &Value, at: DateTime<Utc>) -> GatewayResult<usize> {
    let quote = ctx.text("venue.quote_asset")?;
    let mut map = parse_list(data, &quote)?;
    if ignored(ctx)? {
        strip(&mut map);
    }
    let n = map.len();
    symbols.load(map, at);
    Ok(n)
}
