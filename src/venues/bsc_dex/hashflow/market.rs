//! Рынок Hashflow Taker API v3. Уровни каждого maker заданы порциями, первый объём задаёт минимум.
//! https://docs.hashflow.com/hashflow/taker/getting-started-api-v3
use super::{Ctx, wire::Chain};
use crate::gateway::hashflow::Request;
use crate::gateway::http;
use alloy_primitives::Address;
use reqwest::{
    RequestBuilder,
    header::{AUTHORIZATION, HeaderValue},
};
use rust_decimal::Decimal;
use serde::{Deserialize, de::DeserializeOwned};
use std::collections::{BTreeMap, BTreeSet};

/// Передаёт ключ только официальному HTTP API. Тела подписанных ответов не сохраняются.
pub(super) async fn read<T: DeserializeOwned>(ctx: &Ctx, req: RequestBuilder) -> Result<T, String> {
    let key = ctx.vault.get("hashflow.taker_key").map_err(|e| e.message)?;
    let mut header =
        HeaderValue::from_str(key.as_str()).map_err(|_| "Неверный формат ключа Hashflow")?;
    header.set_sensitive(true);
    let (status, body) = http::exchange(
        req.header(AUTHORIZATION, header),
        ctx.limits().map_err(|e| e.message)?,
    )
    .await
    .map_err(|_| "Нет полного ответа Hashflow")?;
    if !status.is_success() {
        return Err(format!("Hashflow вернул HTTP {status}"));
    }
    serde_json::from_str(&body).map_err(|_| "Ответ Hashflow не соответствует контракту API".into())
}

/// Экранирует значения source, maker и chain в запросе к официальной конечной точке.
fn market_url(path: &str, source: &str, chain: u64) -> Result<reqwest::Url, String> {
    let mut url = reqwest::Url::parse(&format!("https://api.hashflow.com/taker/v3/{path}"))
        .map_err(|_| "Не удалось составить URL Hashflow")?;
    url.query_pairs_mut()
        .append_pair("source", source)
        .append_pair("baseChainType", "evm")
        .append_pair("baseChainId", &chain.to_string());
    Ok(url)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Makers {
    market_makers: Vec<String>,
}

pub(super) async fn makers(
    ctx: &Ctx,
    source: &str,
    chain: u64,
) -> Result<BTreeSet<String>, String> {
    let url = market_url("market-makers", source, chain)?;
    let r: Makers = read(ctx, ctx.http.get(url)).await?;
    Ok(r.market_makers
        .into_iter()
        .filter(|m| !m.trim().is_empty())
        .collect())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Book {
    status: String,
    base_chain: Chain,
    levels: BTreeMap<String, Vec<Market>>,
}
#[derive(Deserialize)]
struct Market {
    pair: Pair,
    levels: Vec<Level>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pair {
    base_token: Address,
    quote_token: Address,
}
#[derive(Deserialize)]
struct Level {
    q: String,
    p: String,
}

/// Рассчитывает выход при заданном объёме. Несколько maker остаются независимыми маршрутами.
fn amount_out(levels: &[Level], amount: Decimal) -> Option<Decimal> {
    let mut remaining = amount;
    let mut out = Decimal::ZERO;
    for (i, level) in levels.iter().enumerate() {
        let qty = Decimal::from_str_exact(&level.q).ok()?;
        let price = Decimal::from_str_exact(&level.p).ok()?;
        if qty <= Decimal::ZERO || price <= Decimal::ZERO || (i == 0 && amount < qty) {
            return None;
        }
        let part = remaining.min(qty);
        out = out.checked_add(part.checked_mul(price)?)?;
        remaining -= part;
        if remaining.is_zero() {
            return Some(out);
        }
    }
    None
}

/// Запрашивает подходящих по ликвидности maker и наибольший ориентировочный выход для сравнения.
pub(super) async fn levels(
    ctx: &Ctx,
    source: &str,
    chain: u64,
    req: &Request,
    amount: Decimal,
) -> Result<(Vec<String>, Decimal), String> {
    let known = makers(ctx, source, chain).await?;
    if known.is_empty() {
        return Err("Нет маркетмейкеров Hashflow на выбранной сети".into());
    }
    let mut url = market_url("price-levels", source, chain)?;
    url.query_pairs_mut()
        .append_pair("baseToken", &format!("{:#x}", req.input));
    for maker in &known {
        url.query_pairs_mut().append_pair("marketMakers[]", maker);
    }
    let book: Book = read(ctx, ctx.http.get(url)).await?;
    if book.status != "success" || !book.base_chain.is(chain) {
        return Err("Нет уровней на запрошенной сети".into());
    }
    let mut makers = vec![];
    let mut best = Decimal::ZERO;
    for (name, markets) in book.levels {
        if !known.contains(&name) {
            continue;
        }
        let quote = markets
            .into_iter()
            .filter(|m| m.pair.base_token == req.input && m.pair.quote_token == req.output)
            .filter_map(|m| amount_out(&m.levels, amount))
            .max();
        if let Some(out) = quote {
            makers.push(name);
            best = best.max(out);
        }
    }
    if makers.is_empty() {
        return Err("Нет уровня Hashflow с достаточной ликвидностью для заданной суммы".into());
    }
    Ok((makers, best))
}
