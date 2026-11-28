//! Кандидаты Umia для первого прохода без индексатора. Каждая версия vault имеет собственный PoolId.
//! Перед включением в граф launch_market отдельно сверяет менеджер, хук и связи vault на узле.
use super::{
    families::{self, Family},
    family_variant::Variant,
    key::PoolKey,
    launch_keys,
    stable_list::{self, Extra},
};
use crate::{gateway::errors::GatewayResult, venues::bsc_dex::ctx::Ctx};
use alloy_primitives::{Address, Bytes};
use std::collections::HashMap;

pub(super) fn extend(
    ctx: &Ctx,
    profiles: &[&Family],
    coins: &HashMap<Address, String>,
    quotes: &[Address],
    out: &mut Extra,
) {
    extend_with(ctx, profiles, coins, quotes, out, |id| ctx.dex_list("dex.launch_pool_keys", id));
}

fn extend_with(
    ctx: &Ctx,
    profiles: &[&Family],
    coins: &HashMap<Address, String>,
    quotes: &[Address],
    out: &mut Extra,
    mut keys_for: impl FnMut(&str) -> GatewayResult<Vec<String>>,
) {
    for profile in profiles {
        // Между составлением плана и чтением ключей профиль могли выключить или сменить его вид.
        let f = match families::classify(ctx, profile.id) {
            Ok(f) if f.variant == Variant::LaunchUmia => f,
            Ok(_) => continue,
            Err(reason) => {
                out.notes.push(reason);
                continue;
            }
        };
        let (Ok(keys), Ok(roles)) = (keys_for(f.id), launch_keys::roles(ctx, f)) else {
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
