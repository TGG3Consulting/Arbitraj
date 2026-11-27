//! Роли с проверенным байткодом и два обработчика обмена в хранилище CrocSwapDex на каноническом блоке.
use super::{ICroc, bad, format::{FLAG_CROSS_CALLPATH, SWAP_CALLPATH, proxy_slot}};
use crate::{
    gateway::errors::GatewayResult,
    venues::bsc_dex::{ctx::Ctx, node::call, pools::{
        balancer_v1::{data, views}, deployment_at_head::Cache, families::Family, solidly_classic_factory::plain,
    }},
};
use alloy_primitives::{Address, B256, U256};
use serde_json::Value;
use std::sync::LazyLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Roles {
    pub dex: Address,
    pub query: Address,
    pub impact: Address,
    pub hot: Address,
    pub knockout: Address,
    pub hashes: [B256; 5],
}
fn roles(ctx: &Ctx, f: &Family) -> GatewayResult<Roles> {
    let mut rows: [Option<(Address, B256)>; 5] = [None; 5];
    for row in ctx.dex_list("dex.ambient_roles", f.id)? {
        let fields: Vec<_> = row.split_whitespace().collect();
        let [name, a, h] = fields.as_slice() else {
            return Err(bad(ctx));
        };
        let i = ["dex", "query", "impact", "hot", "knockout"]
            .iter()
            .position(|s| s == name)
            .ok_or_else(|| bad(ctx))?;
        let (a, h): (Address, B256) = (
            a.parse().map_err(|_| bad(ctx))?,
            h.parse().map_err(|_| bad(ctx))?,
        );
        if a.is_zero() || h.is_zero() || rows[i].replace((a, h)).is_some() {
            return Err(bad(ctx));
        }
    }
    let [Some(dex), Some(query), Some(impact), Some(hot), Some(ko)] = rows else {
        return Err(bad(ctx));
    };
    if dex.0 != ctx.dex_addr("dex.factory", f.id)?
        || query.0 != ctx.dex_addr("dex.ambient_query", f.id)?
        || impact.0 != ctx.dex_addr("dex.quoter", f.id)?
    {
        return Err(bad(ctx));
    }
    Ok(Roles {
        dex: dex.0,
        query: query.0,
        impact: impact.0,
        hot: hot.0,
        knockout: ko.0,
        hashes: [dex.1, query.1, impact.1, hot.1, ko.1],
    })
}
static CACHE: LazyLock<Cache<(), (B256, Roles)>> = LazyLock::new(Default::default);
pub(crate) async fn bind(ctx: &Ctx, f: &Family, at: &Value) -> GatewayResult<Roles> {
    let r = roles(ctx, f)?;
    let h = at["blockHash"]
        .as_str()
        .and_then(|v| v.parse().ok())
        .filter(|_| at["requireCanonical"] == true)
        .ok_or_else(|| bad(ctx))?;
    let cell = CACHE.at((h, r));
    cell.get_or_try_init(|| async {
        let calls = [
            data(r.query, ICroc::dex_Call {}),
            data(r.impact, ICroc::dex_Call {}),
            data(
                r.dex,
                ICroc::readSlotCall {
                    slot: proxy_slot(SWAP_CALLPATH),
                },
            ),
            data(
                r.dex,
                ICroc::readSlotCall {
                    slot: proxy_slot(FLAG_CROSS_CALLPATH),
                },
            ),
        ];
        let (v, _, _, _, _, _) = tokio::try_join!(
            views(ctx, &calls, at),
            plain(ctx, r.dex, r.hashes[0], at),
            plain(ctx, r.query, r.hashes[1], at),
            plain(ctx, r.impact, r.hashes[2], at),
            plain(ctx, r.hot, r.hashes[3], at),
            plain(ctx, r.knockout, r.hashes[4], at)
        )?;
        if v.len() != 4
            || call::decode::<ICroc::dex_Call>(&v[0])? != r.dex
            || call::decode::<ICroc::dex_Call>(&v[1])? != r.dex
            || call::decode::<ICroc::readSlotCall>(&v[2])? != U256::from_be_slice(r.hot.as_slice())
            || call::decode::<ICroc::readSlotCall>(&v[3])?
                != U256::from_be_slice(r.knockout.as_slice())
        {
            return Err(bad(ctx));
        }
        Ok(())
    })
    .await?;
    Ok(r)
}
