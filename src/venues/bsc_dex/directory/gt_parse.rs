//! Файл: venues/bsc_dex/directory/gt_parse.rs
//! Что делает: разбор ответов индексатора GeckoTerminal / CoinGecko on-chain (формат JSON:API): страница пулов
//! монеты (data[] + included[] с токенами и DEX), страница списка DEX сети (data[] + links.next) и текст ошибки
//! (errors[].title). Только разбор — без сети.
//! Кто вызывает: directory/gt_http.rs, directory/refresh.rs, directory/check.rs, tests/bsc_dex_directory.rs.
//! От чего зависит: core/types (IndexerDex), serde, rust_decimal, alloy-primitives.
//! Настройки: нет.
//!
//! Официальное описание — https://docs.coingecko.com/demo/reference/top-pools-contract-address
//! (GET /onchain/networks/{network}/tokens/{token_address}/pools: data[].attributes.address, reserve_in_usd;
//! relationships base_token / quote_token / dex; include=base_token,quote_token,dex — объекты в included[]: address,
//! symbol, name) и https://docs.coingecko.com/demo/reference/endpoint-overview (GET /onchain/networks/{network}/dexes).
//! Сверено с записанными ответами 2026-10-24 (tests/data/indexers_2026-10-24): у пулов V4/Infinity address —
//! PoolId (0x + 64 знака), у V2/V3 — адрес контракта; у списка DEX есть links.next (null на последней странице);
//! страница за пределами — HTTP 400 {"errors":[{"status":"400","title":"expected :page in 1..2; got 3"}]};
//! у ответа пулов links нет.

use std::collections::HashMap;
use std::str::FromStr;

use alloy_primitives::Address;
use rust_decimal::Decimal;
use serde::Deserialize;

use crate::core::types::IndexerDex;

/// Пул из ответа индексатора — как он есть, до проверки на своём узле.
#[derive(Debug, Clone, PartialEq, serde::Serialize, Deserialize)]
pub struct IndexedPool {
    pub dex_id: String,
    /// Адрес пула или PoolId — строкой в нижнем регистре, как отдал индексатор.
    pub pool: String,
    pub base: Address,
    pub quote: Address,
    pub base_symbol: String,
    pub quote_symbol: String,
    pub reserve_usd: Option<Decimal>,
}

#[derive(Deserialize)]
struct Page {
    data: Vec<Item>,
    #[serde(default)]
    included: Vec<Included>,
}

#[derive(Deserialize)]
struct Item {
    attributes: Attrs,
    relationships: Rels,
}

#[derive(Deserialize)]
struct Attrs {
    address: String,
    reserve_in_usd: Option<String>,
}

#[derive(Deserialize)]
struct Rels {
    base_token: Rel,
    quote_token: Rel,
    dex: Rel,
}

#[derive(Deserialize)]
struct Rel {
    data: RelData,
}

#[derive(Deserialize)]
struct RelData {
    id: String,
}

#[derive(Deserialize)]
struct Included {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    attributes: IncAttrs,
}

#[derive(Deserialize)]
struct IncAttrs {
    address: Option<String>,
    symbol: Option<String>,
    name: Option<String>,
}

/// Страница пулов монеты. Принимает: тело ответа. Возвращает: пулы по порядку ответа.
/// Ошибка — текст: тело не по формату, у пула нет токена в included или адрес токена не адрес.
pub fn pools(body: &str) -> Result<Vec<IndexedPool>, String> {
    let page: Page = serde_json::from_str(body).map_err(|e| format!("не по формату: {e}"))?;
    let tokens: HashMap<&str, &IncAttrs> = page
        .included
        .iter()
        .filter(|i| i.kind == "token")
        .map(|i| (i.id.as_str(), &i.attributes))
        .collect();
    let token = |id: &str| -> Result<(Address, String), String> {
        let t = tokens
            .get(id)
            .ok_or_else(|| format!("токена {id} нет в included"))?;
        let a = t.address.as_deref().unwrap_or_default();
        let addr = Address::from_str(a).map_err(|_| format!("токен {id}: «{a}» — не адрес"))?;
        Ok((addr, t.symbol.clone().unwrap_or_default()))
    };
    let mut out = Vec::with_capacity(page.data.len());
    for item in page.data {
        let (base, base_symbol) = token(&item.relationships.base_token.data.id)?;
        let (quote, quote_symbol) = token(&item.relationships.quote_token.data.id)?;
        let reserve = item.attributes.reserve_in_usd.as_deref();
        out.push(IndexedPool {
            dex_id: item.relationships.dex.data.id,
            pool: item.attributes.address.to_lowercase(),
            base,
            quote,
            base_symbol,
            quote_symbol,
            reserve_usd: reserve.and_then(|r| Decimal::from_str(r).ok()),
        });
    }
    Ok(out)
}

#[derive(Deserialize)]
struct DexPage {
    data: Vec<DexItem>,
    links: Option<Links>,
}

#[derive(Deserialize)]
struct DexItem {
    id: String,
    attributes: IncAttrs,
}

#[derive(Deserialize)]
struct Links {
    next: Option<String>,
}

/// Страница списка DEX сети. Принимает: тело. Возвращает: (DEX страницы, есть ли следующая — links.next не null).
/// Ошибка — текст: тело не по формату.
pub fn dexes(body: &str) -> Result<(Vec<IndexerDex>, bool), String> {
    let page: DexPage = serde_json::from_str(body).map_err(|e| format!("не по формату: {e}"))?;
    let list = page
        .data
        .into_iter()
        .map(|d| IndexerDex {
            name: d.attributes.name.unwrap_or_else(|| d.id.clone()),
            id: d.id,
        })
        .collect();
    let next = page.links.and_then(|l| l.next).is_some();
    Ok((list, next))
}

#[derive(Deserialize)]
struct Errors {
    errors: Vec<ErrorItem>,
}

#[derive(Deserialize)]
struct ErrorItem {
    title: Option<String>,
}

/// Текст ошибки индексатора (errors[].title через «; »). None — тело не в формате ошибки. Ошибок нет.
pub fn error_title(body: &str) -> Option<String> {
    let e: Errors = serde_json::from_str(body).ok()?;
    let list: Vec<String> = e.errors.into_iter().filter_map(|i| i.title).collect();
    (!list.is_empty()).then(|| list.join("; "))
}
