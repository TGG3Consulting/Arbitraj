//! Файл: venues/kraken/methods.rs
//! Что делает: чтение справочников Kraken для сетей монет: методы ввода/вывода Funding (Beta) — все страницы по
//! next_cursor (или одной монеты) и разбор строки метода, и активы спота (статус и знаки) GET /0/public/Assets.
//! Кто вызывает: wallet.rs.
//! От чего зависит: rest.rs, num.rs, errors.rs, ctx.rs.
//! Настройки: нет.
//!
//! Официально:
//! - GET /funding/v1/methods/{direction} (https://docs.kraken.com/api-reference/funding-beta/list-funding-methods.md):
//!   direction deposit | withdraw; asset[class] (currency | tokenized_asset) и asset[name] — фильтр; limit ≤ 10000;
//!   cursor — «Do not provide other request parameters with a cursor»; ответ {methods[], next_cursor} (нет — страниц
//!   больше нет); право «Funds permissions - Query»;
//! - GET /0/public/Assets (OpenAPI spot-rest.yaml): result «имя → {aclass, altname, decimals, display_decimals,
//!   status}», status — enabled, deposit_only, withdrawal_only, funding_temporarily_disabled; asset — фильтр.

use std::collections::HashMap;

use reqwest::Method as Http;
use rust_decimal::Decimal;
use serde_json::Value;

use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::errors::Op;
use super::{num, rest};

/// Класс актива для криптовалют — значение перечня asset[class] («currency»).
pub const CLASS: &str = "currency";

/// Направление метода Funding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Deposit,
    Withdraw,
}

impl Dir {
    /// Значение direction в пути. Ошибок нет.
    pub fn path(self) -> &'static str {
        match self {
            Dir::Deposit => "deposit",
            Dir::Withdraw => "withdraw",
        }
    }
}

/// Метод одного направления, разобранный (комиссии и минимумы — в монете метода).
#[derive(Debug, Clone)]
pub struct Method {
    pub id: String,
    pub min: Option<Decimal>,
    pub max: Option<Decimal>,
    /// fees.base.amount (нет — 0).
    pub base_fee: Decimal,
    /// fees.percentage (нет — 0).
    pub pct: Decimal,
    pub contract: Option<String>,
    /// deposit.address_setup_fee больше нуля.
    pub paid_setup: bool,
}

/// Что делает: строка methods → (монета, network_name, метод). None — у метода нет network (не сеть блокчейна).
/// Ошибка BadResponse — нет обязательного поля или число не число.
pub fn parse_method(r: &Value) -> GatewayResult<Option<(String, String, Method)>> {
    let ticker = r
        .get("asset")
        .map(|a| num::text(a, "name"))
        .transpose()?
        .unwrap_or_default();
    let Some(n) = r.get("network") else {
        return Ok(None);
    };
    let fees = r.get("fees");
    let m = Method {
        id: num::text(r, "method_id")?,
        min: num::dec_opt(r, "minimum_amount")?,
        max: num::dec_opt(r, "maximum_amount")?,
        base_fee: fees
            .and_then(|f| f.get("base"))
            .map(num::amount_of)
            .transpose()?
            .map_or(Decimal::ZERO, |(_, a)| a),
        pct: fees
            .map(|f| num::dec_opt(f, "percentage"))
            .transpose()?
            .flatten()
            .unwrap_or_default(),
        contract: num::text_opt(n, "contract_address"),
        paid_setup: r
            .get("deposit")
            .and_then(|d| d.get("address_setup_fee"))
            .map(num::amount_of)
            .transpose()?
            .is_some_and(|(_, a)| a > Decimal::ZERO),
    };
    Ok(Some((ticker, num::text(n, "network_name")?, m)))
}

/// Актив спота из Assets: статус и знаки.
#[derive(Debug, Clone)]
pub struct AssetInfo {
    pub status: String,
    pub decimals: Option<u32>,
}

/// Что делает: методы направления, все страницы. Принимает: окружение, направление, монету (None — все).
/// Возвращает: строки methods всех страниц. Ошибки: ключ, права (Funds — Query), связь, BadResponse.
pub async fn list(ctx: &Ctx, dir: Dir, ticker: Option<&str>) -> GatewayResult<Vec<Value>> {
    let path = format!("/funding/v1/methods/{}", dir.path());
    let mut query = match ticker {
        Some(t) => vec![rest::p("asset[class]", CLASS), rest::p("asset[name]", t)],
        None => vec![],
    };
    let mut out = Vec::new();
    loop {
        let mut v = rest::funding(ctx, Http::GET, &path, query.clone(), None, Op::Other).await?;
        let rows = v
            .get_mut("methods")
            .and_then(Value::as_array_mut)
            .map(std::mem::take)
            .ok_or_else(|| super::errors::bad(&format!("{path}: нет methods"), &v.to_string()))?;
        out.extend(rows);
        match num::text_opt(&v, "next_cursor") {
            // Тот же курсор второй раз — ответ зациклен: не читать бесконечно.
            Some(c) if query.first().is_some_and(|(k, x)| k == "cursor" && *x == c) => {
                return Err(super::errors::bad(
                    &format!("{path}: next_cursor повторился"),
                    &c,
                ));
            }
            // «Do not provide other request parameters with a cursor».
            Some(c) => query = vec![rest::p("cursor", c)],
            None => return Ok(out),
        }
    }
}

/// Что делает: активы спота (статус, знаки). Принимает: окружение, монету (None — все).
/// Возвращает: имя и altname → AssetInfo. Ошибки: связь, error[] Kraken.
pub async fn assets(ctx: &Ctx, ticker: Option<&str>) -> GatewayResult<HashMap<String, AssetInfo>> {
    let q = ticker
        .map(|t| vec![rest::p("asset", t)])
        .unwrap_or_default();
    let res = rest::public(ctx, "Assets", q).await?;
    Ok(parse_assets(&res))
}

/// Разбор result Assets: ключ и altname → статус и знаки. Неразборчивые строки пропускаются. Ошибок нет.
pub fn parse_assets(res: &Value) -> HashMap<String, AssetInfo> {
    let mut out = HashMap::new();
    for (name, a) in res.as_object().into_iter().flatten() {
        let info = AssetInfo {
            status: num::text_opt(a, "status").unwrap_or_default(),
            decimals: a
                .get("decimals")
                .and_then(Value::as_u64)
                .and_then(|d| u32::try_from(d).ok())
                .map(|d| d.min(28)),
        };
        if let Some(alt) = num::text_opt(a, "altname") {
            out.entry(alt).or_insert_with(|| info.clone());
        }
        out.insert(name.clone(), info);
    }
    out
}
