//! Official HTTPS JSON-RPC. A configured URL is the operator's selected maker. No registry-provided URL is contacted automatically.
use super::{no, profile::Profile, wire::Order};
use crate::{
    gateway::{errors::GatewayResult, http},
    venues::bsc_dex::ctx::Ctx,
};
use alloy_primitives::{Address, U256};
use reqwest::{Client, Url};
use serde_json::{Value, json};
#[derive(Clone, Copy)]
pub(super) struct Query {
    pub input: Address,
    pub output: Address,
    pub amount: U256,
    pub exact_out: bool,
}
pub(super) struct Maker {
    client: Client,
    url: Url,
}
impl Maker {
    pub(super) fn endpoints(ctx: &Ctx) -> Vec<Self> {
        let Ok(urls) = ctx.settings.list("airswap.makers", ctx.at()) else {
            return vec![];
        };
        let Ok(client) = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .build()
        else {
            return vec![];
        };
        urls.iter()
            .filter_map(|v| {
                let url = Url::parse(v).ok()?;
                (url.scheme() == "https"
                    && url.host().is_some()
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.fragment().is_none()
                    && url.query().is_none())
                .then(|| Self {
                    client: client.clone(),
                    url,
                })
            })
            .collect()
    }
    async fn rpc(&self, ctx: &Ctx, method: &str, params: Value) -> GatewayResult<Option<Value>> {
        let bad = || {
            no(
                ctx,
                "AirSwap: сервер maker не вернул ожидаемый ответ JSON-RPC",
            )
        };
        let request = self
            .client
            .post(self.url.clone())
            .json(&json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}));
        let (status, body) = http::exchange(request, ctx.limits()?)
            .await
            .map_err(|_| bad())?;
        if !status.is_success() {
            return Err(no(ctx, &format!("AirSwap: HTTP {}", status.as_u16())));
        }
        let v: Value = serde_json::from_str(&body).map_err(|_| bad())?;
        if v["id"] != 1 || v["jsonrpc"] != "2.0" {
            return Err(bad());
        }
        if v["error"]["code"] == -32601 {
            return Ok(None);
        }
        if !v["error"].is_null() {
            return Err(bad());
        }
        Ok(Some(v.get("result").cloned().ok_or_else(bad)?))
    }
    pub(super) async fn supports(&self, ctx: &Ctx, q: Query, p: Profile) -> GatewayResult<bool> {
        let (protocols, tokens) = tokio::try_join!(
            self.rpc(ctx, "getProtocols", json!([])),
            self.rpc(ctx, "getTokens", json!([]))
        )?;
        let parse = |v: &Value| v.as_str().and_then(|s| s.parse::<Address>().ok());
        let supported = protocols.as_ref().is_none_or(|v| {
            v.as_array().is_some_and(|rows| {
                rows.iter().any(|r| {
                    if r.as_str() == Some("0x02ad05d3") {
                        return true;
                    }
                    let terms = &r["params"];
                    let chain = terms["chainId"]
                        .as_u64()
                        .or_else(|| terms["chainId"].as_str()?.parse().ok());
                    r["interfaceId"] == "0x02ad05d3"
                        && chain == Some(p.chain)
                        && parse(&terms["swapContractAddress"]) == Some(p.exchange.address)
                })
            })
        });
        Ok(supported
            && tokens.as_ref().is_none_or(|v| {
                v.as_array().is_some_and(|rows| {
                    [q.input, q.output]
                        .iter()
                        .all(|t| rows.iter().any(|v| parse(v) == Some(*t)))
                })
            }))
    }
    pub(super) async fn order(
        &self,
        ctx: &Ctx,
        q: Query,
        p: Profile,
        sender: Address,
        ms: u64,
    ) -> GatewayResult<Order> {
        let mut params = json!({"chainId":p.chain.to_string(),"swapContract":p.exchange.address,"signerToken":q.output,"senderToken":q.input,"senderWallet":sender,"minExpiry":ms.div_ceil(1000).to_string()});
        let (method, field) = if q.exact_out {
            ("getSenderSideOrderERC20", "signerAmount")
        } else {
            ("getSignerSideOrderERC20", "senderAmount")
        };
        params[field] = json!(q.amount.to_string());
        let v = self
            .rpc(ctx, method, params)
            .await?
            .ok_or_else(|| no(ctx, "AirSwap: метод RFQ недоступен"))?;
        if v.get("chainId")
            .is_some_and(|c| c.as_u64().or_else(|| c.as_str()?.parse().ok()) != Some(p.chain))
            || v.get("swapContract").is_some_and(|v| {
                v.as_str().and_then(|s| s.parse::<Address>().ok()) != Some(p.exchange.address)
            })
        {
            return Err(no(ctx, "AirSwap: чужой домен ответа"));
        }
        Order::read(&v)
            .filter(|o| {
                (o.input, o.output) == (q.input, q.output)
                    && (if q.exact_out { o.quoted } else { o.amount }) == q.amount
                    && o.sender.is_none_or(|s| s == sender || s.is_zero())
            })
            .ok_or_else(|| {
                no(
                    ctx,
                    "AirSwap: условия подписанного ордера отличаются от запроса",
                )
            })
    }
    pub(super) async fn level(&self, ctx: &Ctx, q: Query) -> GatewayResult<Option<(U256, U256)>> {
        let (base, quote) = if q.exact_out {
            (q.output, q.input)
        } else {
            (q.input, q.output)
        };
        let prices = self
            .rpc(
                ctx,
                "getPricingERC20",
                json!({"pairs":[{"baseToken":base,"quoteToken":quote}]}),
            )
            .await?;
        let n = |v: &Value| {
            v.as_str()
                .and_then(|s| U256::from_str_radix(s, 10).ok())
                .or_else(|| v.as_u64().map(U256::from))
        };
        let result = prices
            .as_ref()
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|r| {
                let address = |key: &str| r[key].as_str().and_then(|s| s.parse::<Address>().ok());
                address("baseToken") == Some(base)
                    && address("quoteToken") == Some(quote)
                    && r.get("minimum")
                        .is_none_or(|v| n(v).is_some_and(|v| v <= q.amount))
            })
            .filter_map(|r| r[if q.exact_out { "ask" } else { "bid" }].as_array())
            .flatten()
            .filter_map(|l| {
                let [b, c] = l.as_array()?.as_slice() else {
                    return None;
                };
                let (b, c) = (n(b)?, n(c)?);
                (b == q.amount && !c.is_zero()).then_some(if q.exact_out { (c, b) } else { (b, c) })
            })
            .reduce(|a, b| {
                if (q.exact_out && a.0 < b.0) || (!q.exact_out && a.1 > b.1) {
                    a
                } else {
                    b
                }
            });
        Ok(result)
    }
}
