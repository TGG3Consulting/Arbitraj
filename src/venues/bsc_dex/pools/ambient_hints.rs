//! Запрос списка физических ключей GraphCache. Инициализацию каждого кандидата проверяет ambient_directory.
use super::{Ctx, Family, GatewayResult, Market, bad};
use crate::gateway::http;
use alloy_primitives::Address;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Hint {
    base: Address,
    quote: Address,
    pool_idx: Box<serde_json::value::RawValue>,
}
pub(super) async fn hints(ctx: &Ctx, f: &Family, chain: u64) -> GatewayResult<Vec<Market>> {
    let mut url = reqwest::Url::parse(&ctx.dex_text("dex.ambient_index_url", f.id)?)
        .map_err(|_| bad(ctx))?
        .join("pool_list")
        .map_err(|_| bad(ctx))?;
    if url.scheme() != "https" || url.host().is_none() {
        return Err(bad(ctx));
    }
    url.query_pairs_mut()
        .append_pair("chainId", &format!("{chain:#x}"));
    let (status, body) = http::exchange(ctx.http.get(url), ctx.limits()?)
        .await
        .map_err(|_| bad(ctx))?;
    if !status.is_success() {
        return Err(bad(ctx));
    }
    #[derive(Deserialize)]
    struct Reply {
        data: Vec<Hint>,
    }
    let reply: Reply = serde_json::from_str(&body).map_err(|_| bad(ctx))?;
    Ok(reply
        .data
        .into_iter()
        .filter_map(|v| {
            let raw = v.pool_idx.get();
            let index = if raw.starts_with('"') {
                serde_json::from_str::<String>(raw).ok()?.parse().ok()?
            } else {
                raw.parse().ok()?
            };
            (v.base < v.quote).then_some(Market {
                base: v.base,
                quote: v.quote,
                index,
            })
        })
        .collect())
}
