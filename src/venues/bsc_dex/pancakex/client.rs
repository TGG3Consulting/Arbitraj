//! Два отдельных запроса PCSX. Запрос цены никогда не публикует подпись или заявку.
use super::{Request, Terms, profile::Profile};
use crate::{
    gateway::http::{self, Limits},
    venues::bsc_dex::{ctx::Ctx, ryze},
};
use alloy_primitives::{Address, B256, Bytes, U256};
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::OnceLock;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Offer {
    pub filler: Address,
    pub id: String,
    pub version: String,
    pub encoded: Bytes,
    pub hash: B256,
}
impl Offer {
    pub fn read(msg: &Value) -> Option<Self> {
        if msg["messageType"] != "PRICE_RESPONSE" {
            return None;
        }
        let b = &msg["message"]["bestOrder"];
        if b["type"] != "DUTCH_LIMIT" {
            return None;
        }
        let order = &b["order"];
        let id = order["quoteId"].as_str()?;
        if id.is_empty() || id.len() > 50 {
            return None;
        }
        let filler = order["orderInfo"]["exclusiveFiller"]
            .as_str()?
            .parse()
            .ok()?;
        let s = Self {
            filler,
            id: id.into(),
            version: order["rfqVersion"].as_str()?.into(),
            encoded: order["encodedOrder"].as_str()?.parse().ok()?,
            hash: order["orderHash"].as_str()?.parse().ok()?,
        };
        (s.encoded.len() <= 4000 && !s.hash.is_zero()).then_some(s)
    }
    pub fn terms(&self, p: &Profile, r: &Request) -> Option<Terms> {
        let t = p.roles().terms(&self.version, &self.encoded, r)?;
        (self.hash == t.order_hash).then_some(t)
    }
}
#[derive(Clone, Copy)]
pub struct Query {
    pub input: Address,
    pub output: Address,
    pub owner: Address,
    pub size: U256,
    pub buy: bool,
}
impl Query {
    pub fn body(self, chain: u64, slippage_bps: u64) -> Value {
        let pct = format!("{}.{:02}", slippage_bps / 100, slippage_bps % 100);
        json!({"tokenInChainId":chain,"tokenOutChainId":chain,"tokenIn":self.input,"tokenOut":self.output,
            "amount":self.size.to_string(),"type":if self.buy{"EXACT_OUTPUT"}else{"EXACT_INPUT"},"slippageTolerance":pct,
            "configs":[{"routingType":"DUTCH_LIMIT","useSyntheticQuotes":false,"swapper":self.owner,"recipient":self.owner}]})
    }
}
fn client() -> Option<&'static Client> {
    static C: OnceLock<Result<Client, reqwest::Error>> = OnceLock::new();
    C.get_or_init(|| {
        Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .build()
    })
    .as_ref()
    .ok()
}
pub fn url(base: &str, path: &str) -> Option<Url> {
    let u = ryze::http::endpoint(base, path)?;
    (u.host_str() == Some("x.pancakeswap.com")
        && u.port().is_none()
        && u.path() == format!("/{path}"))
    .then_some(u)
}
pub async fn quote(ctx: &Ctx, base: &str, query: Query, slippage_bps: u64) -> Option<Offer> {
    let path = url(base, "order-price/get-price")?;
    let mut lim = ctx.limits().ok()?;
    lim.max_bytes = lim.max_bytes.min(131072);
    let (status, v) = http::exchange(
        client()?
            .post(path)
            .json(&query.body(ctx.chain_id()?, slippage_bps)),
        lim,
    )
    .await
    .ok()?;
    if !status.is_success() {
        return None;
    }
    Offer::read(&serde_json::from_str(&v).ok()?)
}
/// При потере ответа исходный hash уже закреплён в журнале. Сетевой повтор здесь отсутствует.
pub async fn publish(
    base: &str,
    chain: u64,
    offer: &Offer,
    sig: &alloy::primitives::Signature,
    mut limits: Limits,
) -> Option<B256> {
    let mut target = url(base, "order-handler/order")?;
    target
        .query_pairs_mut()
        .append_pair("strictFirmQuote", "true");
    limits.max_bytes = limits.max_bytes.min(8192);
    let body = json!({"encodedOrder":offer.encoded,"signature":sig.to_string(),"chainId":chain,"quoteId":offer.id});
    let (code, data) = http::exchange(client()?.post(target).json(&body), limits)
        .await
        .ok()?;
    if code != reqwest::StatusCode::CREATED {
        return None;
    }
    let v: Value = serde_json::from_str(&data).ok()?;
    v["hash"]
        .as_str()?
        .parse::<B256>()
        .ok()
        .filter(|h| *h == offer.hash)
}
