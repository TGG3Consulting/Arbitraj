//! Поиск всех пар по принадлежности обеих валют одному зарегистрированному anchor.
use super::{
    bancor_legacy::{self, ILegacy, Roles, read},
    families::{self, Family},
    family_variant::Variant,
    stable_list::Extra,
    verify::{Candidate, Verdict},
};
use crate::{
    core::types::PoolRef,
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, directory::gt_parse::IndexedPool},
};
use alloy_primitives::Address;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

async fn family(
    ctx: &Ctx,
    f: &Family,
    coins: &HashMap<Address, String>,
    quotes: &[Address],
    out: &mut Extra,
) -> GatewayResult<()> {
    let at = super::nami::block(ctx, "latest").await?;
    let r = Roles::read(ctx, f, &at).await?;
    let mut tracked: HashSet<_> = coins.keys().chain(quotes.iter()).copied().collect();
    if tracked.contains(&r.wrapped) {
        tracked.insert(r.native);
    }
    let mut by_asset: HashMap<Address, HashSet<Address>> = HashMap::new();

    for raw in tracked {
        let anchors = read(
            ctx,
            r.registry,
            ILegacy::getConvertibleTokenSmartTokensCall { reserve: raw },
            &at,
        )
        .await?;
        out.calls += 1;

        by_asset
            .entry(r.normalize(raw))
            .or_default()
            .extend(anchors);
    }
    let mut wanted = HashSet::new();
    for coin in coins.keys() {
        for quote in quotes.iter().filter(|q| *q != coin) {
            if let (Some(x), Some(y)) = (by_asset.get(coin), by_asset.get(quote)) {
                wanted.extend(x.intersection(y).copied());
            }
        }
    }
    if wanted.is_empty() {
        return Ok(());
    }
    let pool_anchors: HashSet<_> = read(ctx, r.registry, ILegacy::getLiquidityPoolsCall {}, &at)
        .await?
        .into_iter()
        .collect();
    let anchors: Vec<_> = wanted.intersection(&pool_anchors).copied().collect();
    if anchors.is_empty() {
        return Ok(());
    }
    let converters = read(
        ctx,
        r.registry,
        ILegacy::getConvertersBySmartTokensCall {
            anchors: anchors.clone(),
        },
        &at,
    )
    .await?;
    if anchors.len() != converters.len() {
        return Err(bancor_legacy::bad(ctx));
    }
    let bindings: HashMap<_, _> = anchors.into_iter().zip(converters).collect();
    out.calls += 2;
    for (coin, sym) in coins {
        for q in quotes.iter().filter(|q| *q != coin) {
            let (Some(x), Some(y)) = (by_asset.get(coin), by_asset.get(q)) else {
                continue;
            };
            for cv in x.intersection(y).filter_map(|a| bindings.get(a)) {
                out.found.entry(*coin).or_default().push(IndexedPool {
                    dex_id: f.id.into(),
                    pool: format!("{cv:#x}"),
                    base: *coin,
                    quote: *q,
                    base_symbol: sym.clone(),
                    quote_symbol: coins.get(q).cloned().unwrap_or_else(|| format!("{q:#x}")),
                    reserve_usd: None,
                });
            }
        }
    }
    Ok(())
}
pub(super) async fn scan(ctx: &Ctx, coins: &HashMap<Address, String>, quotes: &[Address]) -> Extra {
    let mut out = Extra::default();
    for f in families::all(ctx) {
        if f.variant == Variant::BancorLegacy
            && families::classify(ctx, f.id).is_ok()
            && let Err(e) = Box::pin(family(ctx, f, coins, quotes, &mut out)).await
        {
            out.no_answer += 1;
            out.notes.push(format!("{}: {}", f.id, e.message));
        }
    }
    out
}
pub(super) async fn verify(ctx: &Ctx, c: &Candidate) -> Verdict {
    let Ok(pool) = c.pool.parse::<Address>() else {
        return Verdict::Rejected("Bancor 2.1: нужен конвертер".into());
    };
    let result = async {
        let at = super::nami::block(ctx, "latest").await?;
        bancor_legacy::market(
            ctx,
            c.family,
            pool,
            super::discover::sorted(c.coin, c.quote),
            &at,
        )
        .await
    }
    .await;
    match result {
        Ok(_) => Verdict::Taken(Box::new(super::verify_woo::found(
            c,
            PoolRef::Contract(pool),
        ))),
        Err(e) if e.code == ErrorCode::NotApplicable => Verdict::Rejected(e.message),
        Err(e) => Verdict::Failed(e.message),
    }
}
pub(super) async fn created(
    ctx: &Ctx,
    f: &'static Family,
    anchor: Address,
    at: &Value,
) -> GatewayResult<(Address, [Address; 2])> {
    let roles = Roles::read(ctx, f, at).await?;
    let pool = read(ctx, anchor, ILegacy::ownerCall {}, at).await?;
    let (a, b) = tokio::try_join!(
        read(
            ctx,
            pool,
            ILegacy::connectorTokensCall {
                i: alloy_primitives::U256::ZERO
            },
            at
        ),
        read(
            ctx,
            pool,
            ILegacy::connectorTokensCall {
                i: alloy_primitives::U256::ONE
            },
            at
        )
    )?;
    Ok((pool, [roles.normalize(a), roles.normalize(b)]))
}
