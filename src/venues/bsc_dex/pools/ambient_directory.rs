//! GraphCache publishes physical keys. Any key, including one discovered from explicit indices,
//! must resolve to an initialized market in the selected singleton on the current canonical head.
use super::{
    ambient::{self, bad},
    balancer_v1::individual,
    families::{self, Family},
    family_variant::Variant,
    stable_list::{self, Extra},
    verify::{Candidate, Verdict},
};
use crate::{
    core::types::{PoolRef, ambient::Market},
    gateway::{
        errors::{ErrorCode, GatewayResult},
        http,
    },
    venues::bsc_dex::ctx::Ctx,
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, U256};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};

pub(crate) type Keys = HashMap<(u64, Address, B256), Market>;
fn indices(ctx: &Ctx, f: &Family) -> Vec<U256> {
    ctx.dex_list("dex.ambient_pool_indices", f.id)
        .unwrap_or_default()
        .iter()
        .filter_map(|s| s.trim().parse().ok())
        .collect()
}
fn pairs(a: Address, b: Address, w: Address) -> Vec<[Address; 2]> {
    if a == b || a.is_zero() || b.is_zero() {
        return vec![];
    }
    let mut out = vec![[a.min(b), a.max(b)]];
    if a == w || b == w {
        out.push([Address::ZERO, if a == w { b } else { a }]);
    }
    out
}
pub(crate) fn resolve(
    ctx: &Ctx,
    f: &Family,
    dex: Address,
    id: B256,
    pair: (Address, Address),
) -> GatewayResult<Market> {
    let chain = ctx.chain_id().ok_or_else(|| bad(ctx))?;
    let w = ctx.addr("dex.native_wrapped")?;
    let allowed = pairs(pair.0, pair.1, w);
    if let Some(m) = ctx
        .index
        .with(|s| s.ambient.get(&(chain, dex, id)).copied())
        && m.id() == id
        && allowed.contains(&[m.base, m.quote])
    {
        return Ok(m);
    }
    for index in indices(ctx, f) {
        for &[base, quote] in &allowed {
            let m = Market { base, quote, index };
            if m.id() == id {
                return Ok(m);
            }
        }
    }
    Err(bad(ctx))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Hint {
    base: Address,
    quote: Address,
    pool_idx: Box<serde_json::value::RawValue>,
}
async fn hints(ctx: &Ctx, f: &Family, chain: u64) -> GatewayResult<Vec<Market>> {
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
pub(super) async fn scan(
    ctx: &Ctx,
    coins: &HashMap<Address, String>,
    quotes: &[Address],
    extra: &mut Extra,
) {
    for f in families::all(ctx)
        .into_iter()
        .filter(|f| f.variant == Variant::Ambient)
    {
        if let Err(e) = scan_family(ctx, f, coins, quotes, extra).await {
            extra.no_answer += 1;
            extra.notes.push(e.message);
        }
    }
}
async fn scan_family(
    ctx: &Ctx,
    f: &Family,
    coins: &HashMap<Address, String>,
    quotes: &[Address],
    extra: &mut Extra,
) -> GatewayResult<()> {
    let block = super::nami::block(ctx, "latest").await?;
    let r = ambient::bind(ctx, f, &block).await?;
    let chain = ctx.chain_id().ok_or_else(|| bad(ctx))?;
    let wrapped = ctx.addr("dex.native_wrapped")?;
    let mut candidates = HashSet::new();
    if let Ok(v) = hints(ctx, f, chain).await {
        candidates.extend(v)
    } else {
        extra.notes.push(format!(
            "{}: индексатор недоступен, проверяются сохранённые пары и заданные индексы",
            f.id
        ))
    }
    candidates.extend(ctx.index.with(|s| {
        s.ambient
            .iter()
            .filter(|((c, d, _), _)| *c == chain && *d == r.dex)
            .map(|(_, m)| *m)
            .collect::<Vec<_>>()
    }));
    for index in indices(ctx, f) {
        for a in coins.keys() {
            for b in quotes {
                for [base, quote] in pairs(*a, *b, wrapped) {
                    candidates.insert(Market { base, quote, index });
                }
            }
        }
    }
    let mut rows: Vec<_> = candidates
        .into_iter()
        .filter_map(|m| {
            let tokens = m.tokens(wrapped)?;
            let wanted = (coins.contains_key(&tokens[0]) && quotes.contains(&tokens[1]))
                || (coins.contains_key(&tokens[1]) && quotes.contains(&tokens[0]));
            wanted.then_some((m, tokens))
        })
        .collect();
    rows.sort_unstable_by_key(|(m, _)| (m.base, m.quote, m.index));
    for group in rows.chunks(super::load_batch::batch(ctx)?) {
        let calls: Vec<_> = group
            .iter()
            .map(|(m, _)| ambient::params(r.query, *m))
            .collect();
        let values = individual(ctx, &calls, &block).await?;
        if values.len() != group.len() {
            return Err(bad(ctx));
        }
        extra.calls += calls.len();
        for ((m, tokens), v) in group.iter().zip(values) {
            let Ok(raw) = v else {
                extra.no_answer += 1;
                continue;
            };
            if !ambient::ICroc::queryPoolParamsCall::abi_decode_returns(&raw)
                .is_ok_and(|p| p.schema == 1 && p.tickSize != 0)
            {
                continue;
            }
            let id = m.id();
            ctx.index.with(|s| s.ambient.insert((chain, r.dex, id), *m));
            for item in stable_list::candidates(f.id, id, tokens, coins, quotes) {
                extra.found.entry(item.base).or_default().push(item);
            }
        }
    }
    Ok(())
}
pub(super) async fn verify(ctx: &Ctx, c: &Candidate) -> Verdict {
    let checked = async {
        let id = c.pool.parse::<B256>().map_err(|_| bad(ctx))?;
        let block = super::nami::block(ctx, "latest").await?;
        let r = ambient::bind(ctx, c.family, &block).await?;
        let m = resolve(ctx, c.family, r.dex, id, (c.coin, c.quote))?;
        ambient::market(ctx, r, m, &block).await?;
        Ok::<_, crate::gateway::errors::GatewayError>(PoolRef::Singleton { manager: r.dex, id })
    }
    .await;
    match checked {
        Ok(r) => Verdict::Taken(Box::new(super::verify_woo::found(c, r))),
        Err(e) if e.code == ErrorCode::QuoteMismatch => Verdict::Rejected(e.message),
        Err(e) => Verdict::Failed(e.message),
    }
}
