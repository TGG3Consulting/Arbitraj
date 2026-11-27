//! Рынок Hashflow Taker API v3. Уровни каждого maker заданы порциями, первый объём задаёт минимум.
//! https://docs.hashflow.com/hashflow/taker/getting-started-api-v3
use super::{Ctx, wire::Chain};
use super::model::Request;
use crate::gateway::market_http;
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
    if key.trim().is_empty() {
        return Err("Нет ключа Hashflow".into());
    }
    let mut header =
        HeaderValue::from_str(key.as_str()).map_err(|_| "Неверный формат ключа Hashflow")?;
    header.set_sensitive(true);
    let limits = ctx.limits().map_err(|e| e.message)?;
    let (status, body) = market_http::exchange("hashflow", req.header(AUTHORIZATION, header), limits)
        .await
        .map_err(|e| e.description(&limits))?;
    if !status.is_success() {
        return Err(format!("Hashflow вернул HTTP {status}"));
    }
    serde_json::from_str(&body).map_err(|_| "Ответ Hashflow не соответствует контракту API".into())
}

/// Экранирует значения source, maker и chain в запросе к официальной конечной точке.
fn market_url(path: &str, source: &str, chain: u64, wallet: Option<Address>) -> Result<reqwest::Url, String> {
    if (source=="api")!=wallet.is_some() || wallet.is_some_and(|a|a.is_zero()){return Err("Hashflow: параметры авторизации противоречат режиму источника".into());}
    let path=if wallet.is_some() && path=="market-makers"{"marketMakers"}else{path};
    let mut url = reqwest::Url::parse(&format!("https://api.hashflow.com/taker/v3/{path}"))
        .map_err(|_| "Не удалось составить URL Hashflow")?;
    url.query_pairs_mut()
        .append_pair("source", source)
        .append_pair("baseChainType", "evm")
        .append_pair("baseChainId", &chain.to_string());
    if let Some(w)=wallet {url.query_pairs_mut().append_pair("wallet",&format!("{w:#x}"));}
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
    wallet:Option<Address>,
) -> Result<BTreeSet<String>, String> {
    let url = market_url("market-makers", source, chain,wallet)?;
    let r: Makers = read(ctx, ctx.http.get(url)).await?;
    Ok(r.market_makers
        .into_iter()
        .filter(|m| !m.trim().is_empty())
        .collect())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Book {
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
fn amount_at(levels: &[Level], amount: Decimal, exact_output: bool) -> Option<Decimal> {
    let mut remaining = amount;
    let mut total = Decimal::ZERO;
    for (i, level) in levels.iter().enumerate() {
        let qty = Decimal::from_str_exact(&level.q).ok()?;
        let price = Decimal::from_str_exact(&level.p).ok()?;
        if qty < Decimal::ZERO || price <= Decimal::ZERO {
            return None;
        }
        let capacity = if exact_output {
            qty.checked_mul(price)?
        } else {
            qty
        };
        if i == 0 && amount < capacity {
            return None;
        }
        let part = remaining.min(capacity);
        total = total.checked_add(if exact_output {
            part.checked_div(price)?
        } else {
            part.checked_mul(price)?
        })?;
        remaining -= part;
        if remaining.is_zero() {
            return Some(total);
        }
    }
    None
}

/// Цена уровня измеряется в выходном токене, количество — во входном.
pub(super) async fn book(
    ctx: &Ctx,
    source: &str,
    chain: u64,
    input: Option<Address>,
    wallet:Option<Address>,
) -> Result<Book, String> {
    let known = makers(ctx, source, chain,wallet).await?;
    if known.is_empty() {
        return Err("Нет маркетмейкеров Hashflow на выбранной сети".into());
    }
    let mut url = market_url("price-levels", source, chain,wallet)?;
    if let Some(token) = input {
        url.query_pairs_mut()
            .append_pair("baseToken", &format!("{token:#x}"));
    }
    for maker in &known {
        url.query_pairs_mut().append_pair("marketMakers[]", maker);
    }
    let book: Book = read(ctx, ctx.http.get(url)).await?;
    if book.status != "success" || !book.base_chain.is(chain) {
        return Err("Нет уровней на запрошенной сети".into());
    }
    let mut book = book;
    book.levels.retain(|name, _| known.contains(name));
    Ok(book)
}

impl Book {
    /// При точном выходе возвращает минимальный вход, при точном входе — максимальный выход.
    pub(super) fn at(
        &self,
        req: &Request,
        amount: Decimal,
        exact_output: bool,
    ) -> Result<(Vec<String>, Decimal), String> {
        let mut names = vec![];
        let mut best: Option<Decimal> = None;
        for (name, markets) in &self.levels {
            for m in markets
                .iter()
                .filter(|m| m.pair.base_token == req.input && m.pair.quote_token == req.output)
            {
                if let Some(value) = amount_at(&m.levels, amount, exact_output) {
                    if names.last() != Some(name) {
                        names.push(name.clone());
                    }
                    best = Some(match best {
                        Some(b) if exact_output => b.min(value),
                        Some(b) => b.max(value),
                        None => value,
                    });
                }
            }
        }
        Ok((
            names,
            best.ok_or("Нет достаточной ликвидности на запрошенный объём")?,
        ))
    }
}

impl Book {
    pub(super) fn markets(
        &self,
        quote: Address,
        tokens: &super::super::tokens::Tokens,
    ) -> BTreeMap<Address, (bool, bool)> {
        let known: std::collections::HashSet<_> =
            tokens.all().into_iter().map(|t| t.address).collect();
        let mut out = BTreeMap::new();
        for m in self.levels.values().flatten() {
            if !m.levels.iter().any(|v| {
                Decimal::from_str_exact(&v.q).is_ok_and(|n| n > Decimal::ZERO)
                    && Decimal::from_str_exact(&v.p).is_ok_and(|n| n > Decimal::ZERO)
            }) {
                continue;
            }
            let (coin, buy) = if m.pair.base_token == quote {
                (m.pair.quote_token, true)
            } else if m.pair.quote_token == quote {
                (m.pair.base_token, false)
            } else {
                continue;
            };
            if coin.is_zero() || coin == quote || !known.contains(&coin) {
                continue;
            }
            let sides = out.entry(coin).or_insert((false, false));
            if buy {
                sides.0 = true;
            } else {
                sides.1 = true;
            }
        }
        out
    }

    pub(super) fn breaks(
        &self,
        input: Address,
        output: Address,
        exact_output: bool,
        max: Decimal,
    ) -> Vec<Decimal> {
        let mut sizes = BTreeSet::new();
        for m in self
            .levels
            .values()
            .flatten()
            .filter(|m| m.pair.base_token == input && m.pair.quote_token == output)
        {
            let mut sum = Decimal::ZERO;
            for lvl in &m.levels {
                let (Ok(q), Ok(p)) = (
                    Decimal::from_str_exact(&lvl.q),
                    Decimal::from_str_exact(&lvl.p),
                ) else {
                    break;
                };
                let Some(step) = (if exact_output {
                    q.checked_mul(p)
                } else {
                    Some(q)
                }) else {
                    break;
                };
                let Some(next) = sum.checked_add(step) else {
                    break;
                };
                if next <= sum || next > max {
                    break;
                }
                sum = next;
                sizes.insert(sum);
            }
        }
        sizes.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Опубликованный пример из раздела 3 Hashflow Taker API v3 (0.5, 1.5, 5 ETH), без секретов и торгового запроса.
    #[test]
    fn published_depth_example_respects_minimum_and_exact_output() {
        let levels: Vec<Level> = serde_json::from_str(
            r#"[{"q":"0.5","p":"3000"},{"q":"1.5","p":"3000"},{"q":"5","p":"2999"}]"#,
        )
        .unwrap();
        let sold = Decimal::from(3u8);
        let proceeds = Decimal::from(8999u16);
        assert_eq!(amount_at(&levels, sold, false), Some(proceeds));
        assert_eq!(amount_at(&levels, proceeds, true), Some(sold));
        assert!(amount_at(&levels, Decimal::new(4999, 4), false).is_none());
        assert!(amount_at(&levels, Decimal::from(1499u16), true).is_none());
        assert!(amount_at(&levels, Decimal::new(7001, 3), false).is_none());
        assert!(amount_at(&levels, Decimal::from(20996u16), true).is_none());
    }
}

#[cfg(test)]
mod credential_tests{
    use super::*;
    #[test]
    fn individual_and_multi_wallet_use_their_documented_identity(){
        let a=Address::repeat_byte(9);
        let maker=market_url("market-makers","api",1,Some(a)).unwrap();
        let levels=market_url("price-levels","api",1,Some(a)).unwrap();
        assert!(maker.path().ends_with("/marketMakers"));
        assert!(levels.path().ends_with("/price-levels"));
        for url in [&maker,&levels]{
            assert!(url.query_pairs().any(|(k,v)|k=="wallet" && v==format!("{a:#x}")));
        }
        let named=market_url("market-makers","partner",8453,None).unwrap();
        assert!(named.path().ends_with("/market-makers"));
        assert!(!named.query_pairs().any(|(k,_)|k=="wallet"));
        assert!(market_url("market-makers","api",1,None).is_err());
        assert!(market_url("market-makers","partner",1,Some(a)).is_err());
        assert!(!super::super::allowed(Some(a),Address::repeat_byte(8)));
    }
}
