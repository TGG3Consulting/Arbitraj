//! Кандидаты Umia для первого прохода без индексатора. Каждая версия vault имеет собственный PoolId.
//! Перед включением в граф launch_market отдельно сверяет менеджер, хук и связи vault на узле.
use super::{
    families,
    family_variant::Variant,
    key::PoolKey,
    launch_keys,
    stable_list::{self, Extra},
};
use crate::venues::bsc_dex::ctx::Ctx;
use alloy_primitives::{Address, Bytes};
use std::collections::HashMap;

pub(super) fn extend(
    ctx: &Ctx,
    coins: &HashMap<Address, String>,
    quotes: &[Address],
    out: &mut Extra,
) {
    let chain = ctx.chain_id().and_then(crate::settings::evm_profiles::by_chain_id);
    if chain.is_none_or(|network| network.chain != "base") {
        return;
    }
    for f in families::all(ctx).into_iter().filter(|f| f.variant == Variant::LaunchUmia) {
        if let Err(reason) = families::classify(ctx, f.id) {
            out.notes.push(reason);
            continue;
        }
        let (Ok(keys), Ok(roles)) = (
            ctx.dex_list("dex.launch_pool_keys", f.id),
            launch_keys::roles(ctx, f),
        ) else {
            continue;
        };
        for text in keys {
            let Some(key) = text.parse::<Bytes>().ok().and_then(|raw| launch_keys::decode(f, &raw)) else {
                out.notes.push(format!("{}: некорректный PoolKey Umia", f.id));
                continue;
            };
            let PoolKey::Uniswap(ref k) = key else { continue };
            if !roles.iter().any(|r| r.kind == "hook" && r.address == k.hooks) {
                continue;
            }
            for row in stable_list::candidates(f.id, key.id(), &[k.currency0, k.currency1], coins, quotes) {
                out.found.entry(row.base).or_default().push(row);
            }
        }
    }
}

#[cfg(test)]
#[path = "umia_seed_tests.rs"]
mod tests;
