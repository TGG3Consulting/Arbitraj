//! Файл: venues/htx/symbols_parse.rs
//! Что делает: разбор ответа GET /v1/settings/common/market-symbols HTX в правила пар (SymbolRules).
//! Кто вызывает: symbols.rs (refresh), tests/htx_real_data.rs.
//! От чего зависит: num.rs, ctx.rs (тикер), symbols.rs (SymbolRules, статусы).
//! Настройки: нет (котируемую монету передаёт symbols.rs).
//!
//! Официально — Get Market Symbols Setting https://huobiapi.github.io/docs/spot/v1/en/#get-market-symbols-setting:
//! symbol, bc, qc (строчными), state (unknown, not-online, pre-online, online, suspend, offline, transfer-board, fuse),
//! pp «price precision», ap «amount precision», vp «value precision», minoa / maxoa «min / max order amount», minov
//! «min order value», lominoa / lomaxoa «min / max amount for limit orders», lomaxba / lomaxsa «max limit buy / sell
//! amount», smminoa / smmaxoa / bmmaxov — рыночные ордера (бот их не ставит), blmlt / slmgt «buy-limit-must-less-than»
//! / «sell-limit-must-greater-than» (от какой цены — не сказано: не проверяются, отказ даст биржа), at «trading by api
//! interface» (значения не перечислены — хранится как есть, проверить на живом ответе). Точность → шаг 10^−n (как Gate).
//! Какие пределы действуют на ордер buy-ioc / sell-ioc (limit или общие), документация не говорит: берётся более строгий
//! из обоих (минимум — больший из minoa и lominoa, максимум — меньший из maxoa, lomaxoa и предела стороны).

use std::collections::HashMap;

use rust_decimal::Decimal;
use serde_json::Value;

use crate::gateway::errors::GatewayResult;

use super::ctx::ticker;
use super::num;
use super::symbols::{SymbolRules, status_code};

/// Наибольшая точность Decimal (28 знаков).
const MAX_SCALE: u32 = 28;

/// Разбор списка: пары с котируемой монетой quote (тикер ядра; ключ — тикер монеты). Пара с неразборчивым полем не
/// валит весь справочник, а попадает в skipped («символ: причина»). Ошибка BadResponse — data не массив.
pub fn parse_list(
    data: &Value,
    quote: &str,
) -> GatewayResult<(HashMap<String, SymbolRules>, Vec<String>)> {
    let list = data
        .as_array()
        .ok_or_else(|| super::errors::bad("market-symbols: data не массив", ""))?;
    let (mut map, mut skipped) = (HashMap::new(), Vec::new());
    for s in list {
        if num::text_opt(s, "qc").map(|q| ticker(&q)).as_deref() != Some(quote) {
            continue;
        }
        match parse(s) {
            Ok(r) => {
                map.insert(r.base.clone(), r);
            }
            Err(e) => skipped.push(format!(
                "{}: {}",
                num::text_opt(s, "symbol").unwrap_or_default(),
                e.message
            )),
        }
    }
    Ok((map, skipped))
}

/// Разбор одной пары. Возвращает: правила. Ошибка BadResponse — нет обязательного поля или точность не целое.
pub fn parse(s: &Value) -> GatewayResult<SymbolRules> {
    let state = num::text(s, "state")?;
    // Строже из двух минимумов — больший.
    let stricter = |a: Option<Decimal>, b: Option<Decimal>| match (a, b) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (x, y) => x.or(y),
    };
    let positive = |f: &str| num::dec_opt(s, f).map(|v| v.filter(|x| *x > Decimal::ZERO));
    let max_all = [positive("maxoa")?, positive("lomaxoa")?]
        .into_iter()
        .flatten()
        .min();
    Ok(SymbolRules {
        symbol: num::text(s, "symbol")?,
        base: ticker(&num::text(s, "bc")?),
        quote: ticker(&num::text(s, "qc")?),
        tick_size: step(s, "pp")?,
        step_size: step(s, "ap")?,
        min_qty: stricter(positive("minoa")?, positive("lominoa")?),
        max_qty: max_all,
        max_buy: positive("lomaxba")?,
        max_sell: positive("lomaxsa")?,
        min_notional: positive("minov")?,
        status: status_code(&state),
        state,
        api: num::text_opt(s, "at"),
    })
}

/// Точность n (знаков после запятой) → шаг 10^−n. Ошибка BadResponse — не целое или больше 28.
fn step(s: &Value, field: &str) -> GatewayResult<Decimal> {
    let n = num::int(s, field)?;
    let n = u32::try_from(n)
        .ok()
        .filter(|n| *n <= MAX_SCALE)
        .ok_or_else(|| super::errors::bad(&format!("{field} = {n} вне 0…28"), &s.to_string()))?;
    Ok(Decimal::new(1, n))
}
