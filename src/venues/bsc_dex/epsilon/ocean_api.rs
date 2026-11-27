//! Публичный GET OpenOcean V4 для открытой цены и unsigned calldata зарегистрированного плательщика.
use super::super::ctx::Ctx;
use alloy_primitives::{Address, Bytes, U256};
use reqwest::Url;
use serde::Deserialize;

pub(super) struct Client<'a> {
    ctx: &'a Ctx,
    base: Url,
    gas: U256,
    chain: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Answer {
    pub in_token: Token,
    pub out_token: Token,
    pub in_amount: String,
    pub out_amount: String,
    pub from: Option<Address>,
    pub to: Option<Address>,
    pub data: Option<Bytes>,
    pub value: Option<String>,
    pub min_out_amount: Option<String>,
    pub chain_id: Option<u64>,
    pub rfq_deadline: Option<u64>,
}
#[derive(Deserialize)]
pub(super) struct Token {
    address: Address,
}
impl<'a> Client<'a> {
    pub fn new(ctx: &'a Ctx, gas: U256) -> Option<Self> {
        let base = Url::parse(&ctx.text("epsilon.openocean_url").ok()?).ok()?;
        if base.scheme() != "https"
            || base.host().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
            || gas.is_zero()
        {
            return None;
        }
        Some(Self {
            ctx,
            base,
            gas,
            chain: ctx.chain_id()?,
        })
    }
    pub async fn request(
        &self,
        input: Address,
        output: Address,
        n: U256,
        account: Option<Address>,
    ) -> Option<Answer> {
        #[derive(Deserialize)]
        struct Reply {
            code: u16,
            data: Answer,
        }
        let mut url = self.base.clone();
        url.path_segments_mut()
            .ok()?
            .pop_if_empty()
            .push(if account.is_some() { "swap" } else { "quote" });
        {
            let mut args = url.query_pairs_mut();
            args.append_pair("inTokenAddress", &format!("{input:#x}"))
                .append_pair("outTokenAddress", &format!("{output:#x}"))
                .append_pair("amountDecimals", &n.to_string())
                .append_pair("gasPriceDecimals", &self.gas.to_string());
            if let Some(payer) = account {
                let a = format!("{payer:#x}");
                args.append_pair("account", &a).append_pair("sender", &a);
            }
        }
        let res = self
            .ctx
            .http
            .get(url)
            .send()
            .await
            .ok()?
            .error_for_status()
            .ok()?;
        if res.content_length().is_some_and(|n| n > 1048576) {
            return None;
        }
        let mut res = res;
        let mut buf = Vec::new();
        while let Some(part) = res.chunk().await.ok()? {
            if buf.len() + part.len() > 1048576 {
                return None;
            }
            buf.extend(part)
        }
        let r: Reply = serde_json::from_slice(&buf).ok()?;
        let d = r.data;
        if r.code != 200
            || d.in_token.address != input
            || d.out_token.address != output
            || d.in_amount.parse::<U256>().ok()? != n
            || d.chain_id.is_some_and(|id| id != self.chain)
        {
            return None;
        }
        Some(d)
    }
}
