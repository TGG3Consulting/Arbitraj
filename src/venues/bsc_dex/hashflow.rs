//! Проверка доступа Hashflow Taker API v3 по запросу оператора. Один multi-wallet
//! source; API использует обычный EVM chain ID для односетевых котировок.
//! https://docs.hashflow.com/hashflow/taker/getting-started-api-v3
use super::ctx::Ctx;
use crate::gateway::{errors::ErrorCode, http};
use reqwest::header::{AUTHORIZATION, HeaderValue};
use serde::Deserialize;

const MARKET_MAKERS: &str = "https://api.hashflow.com/taker/v3/market-makers";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Makers {
    market_makers: Vec<String>,
}

pub(super) async fn connection(ctx: &Ctx) -> Option<Result<String, String>> {
    let source = match ctx.text("hashflow.source") {
        Ok(s) if !s.trim().is_empty() => s,
        Ok(_) => return None,
        Err(e) if e.code == ErrorCode::NotConfigured => return None,
        Err(e) => return Some(Err(e.message)),
    };
    let chain = ctx.chain_id()?;
    if !matches!(chain, 1 | 56 | 8453 | 42161) {
        return None;
    }
    Some(makers(ctx, &source, chain).await)
}

async fn makers(ctx: &Ctx, source: &str, chain: u64) -> Result<String, String> {
    let key = ctx.vault.get("hashflow.taker_key").map_err(|e| e.message)?;
    let mut auth =
        HeaderValue::from_str(key.as_str()).map_err(|_| "неверный формат ключа Hashflow")?;
    auth.set_sensitive(true);
    let req = ctx
        .http
        .get(MARKET_MAKERS)
        .header(AUTHORIZATION, auth)
        .query(&[
            ("source", source),
            ("baseChainType", "evm"),
            ("baseChainId", &chain.to_string()),
        ]);
    let (status, body) = http::exchange(req, ctx.limits().map_err(|e| e.message)?)
        .await
        .map_err(|_| format!("chainId {chain}: нет полного HTTP-ответа Hashflow"))?;
    if !status.is_success() {
        return Err(format!(
            "chainId {chain}: HTTP {status} при запросе маркетмейкеров"
        ));
    }
    let result: Makers = serde_json::from_str(&body)
        .map_err(|_| format!("chainId {chain}: ответ не содержит список marketMakers"))?;
    let count = result
        .market_makers
        .into_iter()
        .filter(|s| !s.trim().is_empty())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    Ok(format!("chainId {chain}: доступно маркетмейкеров {count}"))
}
