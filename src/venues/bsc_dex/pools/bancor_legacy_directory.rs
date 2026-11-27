//! Поиск всех пар по принадлежности обеих валют одному зарегистрированному anchor.
use super::{
    bancor_legacy::{self, ILegacy, Roles, read},
    families::{self, Family},
    family_variant::Variant,
    stable_list::Extra,
    verify::{Candidate, Verdict},
};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, directory::gt_parse::IndexedPool},
};
use alloy_primitives::Address;
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
    let mut wanted = HashSet::new();
    for raw in tracked {
        let anchors = read(
            ctx,
            r.registry,
            ILegacy::getConvertibleTokenSmartTokensCall { reserve: raw },
            &at,
        )
        .await?;
        out.calls += 1;
        wanted.extend(anchors);
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
    let graph = super::bancor_legacy_bridge::edges(ctx, &r, &converters, &at).await?;
    out.calls += 2 + converters.len() * 3;
    for (coin, sym) in coins {
        for q in quotes.iter().filter(|q| *q != coin) {
            for pool in super::bancor_legacy_bridge::paths(&graph, *coin, *q, &r) {
                out.found.entry(*coin).or_default().push(IndexedPool {
                    dex_id: f.id.into(),
                    pool,
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
    let Some(pool) = super::bancor_legacy_bridge::parse(&c.pool) else {
        return Verdict::Rejected("Bancor 2.1: нужен конвертер".into());
    };
    let result = async {
        let at = super::nami::block(ctx, "latest").await?;
        super::bancor_legacy_bridge::bound(
            ctx,
            c.family,
            &pool,
            super::discover::sorted(c.coin, c.quote),
            &at,
        )
        .await
    }
    .await;
    match result {
        Ok(_) => Verdict::Taken(Box::new(super::verify_woo::found(c, pool))),
        Err(e) if e.code == ErrorCode::NotApplicable => Verdict::Rejected(e.message),
        Err(e) => Verdict::Failed(e.message),
    }
}
pub(super) async fn created(
    ctx: &Ctx,
    f: &'static Family,
    anchor: Address,
    coins: &HashMap<Address, String>,
    quotes: &[Address],
) -> GatewayResult<Vec<Candidate>> {
    let at = super::nami::block(ctx, "latest").await?;
    let roles = Roles::read(ctx, f, &at).await?;
    let pool = read(ctx, anchor, ILegacy::ownerCall {}, &at).await?;
    let (a, b) = tokio::try_join!(
        read(
            ctx,
            pool,
            ILegacy::connectorTokensCall {
                i: alloy_primitives::U256::ZERO
            },
            &at
        ),
        read(
            ctx,
            pool,
            ILegacy::connectorTokensCall {
                i: alloy_primitives::U256::ONE
            },
            &at
        )
    )?;
    let touched = [roles.normalize(a), roles.normalize(b)];
    let intersects_quotes = quotes.iter().any(|q| touched.contains(q));
    let subset = coins
        .iter()
        .filter(|(asset, _)| intersects_quotes || touched.contains(asset))
        .map(|(a, t)| (*a, t.clone()))
        .collect::<HashMap<_, _>>();
    if subset.is_empty() {
        return Ok(vec![]);
    }
    let mut result = Extra::default();
    family(ctx, f, &subset, quotes, &mut result).await?;
    let mut candidates = Vec::new();
    let marker = format!("{pool:#x}");
    for rows in result.found.into_values() {
        for row in rows {
            if row.pool.split('/').any(|id| id == marker) {
                candidates.push(Candidate {
                    family: f,
                    pool: row.pool,
                    coin: row.base,
                    quote: row.quote,
                });
            }
        }
    }
    Ok(candidates)
}
