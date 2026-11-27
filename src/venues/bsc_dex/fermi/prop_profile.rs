//! Рынки Lambda, для которых владелец настроил разбор ценового кадра Ethereum.
use super::{Builder, Ctx, prop_oracle::Oracle, slot_config::builders};
use crate::{
    settings::{
        evm_profiles,
        scope::{Scope, check_dex_id},
    },
    venues::bsc_dex::{
        ctx_place::key_of,
        pools::{
            families::{self, Family},
            family_variant::Variant,
        },
    },
};
use std::collections::BTreeMap;

/// Titan и BuilderNet публикуют кадры по часам слотов Ethereum mainnet.
pub(super) fn network(ctx: &Ctx) -> Option<u64> {
    let settings = &ctx.settings;
    let venue = ctx.place.id;
    let profile = evm_profiles::configured(settings, venue)?;
    let scope = Scope::Venue(venue.to_owned());
    let number = settings.get_exact("dex.evm_chain_id", &scope)?.as_u64()?;
    let chain = settings.get_exact("dex.chain", &scope)?;
    (number == profile.evm_chain_id
        && number == evm_profiles::ETHEREUM_MAINNET
        && chain.as_str() == Some(profile.chain))
    .then_some(number)
}

pub(super) struct Profile {
    pub family: &'static Family,
    pub oracle: Oracle,
    pub builders: Vec<Builder>,
}
impl Profile {
    /// Вход API использует локальный id. Только область текущей площадки может дать рынок.
    pub(super) fn lookup(ctx: &Ctx, id: &str) -> Option<Self> {
        if id.contains('@') || check_dex_id(id).is_err() {
            return None;
        }
        let key = key_of(id, ctx.dex_suffix().ok()?.as_deref());
        Self::read(ctx, &key)
    }

    pub(super) fn read(ctx: &Ctx, key: &str) -> Option<Self> {
        let family = families::classify(ctx, key).ok()?;
        if family.variant != Variant::PropVenue {
            return None;
        }
        let market = ctx.dex_addr("dex.factory", key).ok()?;
        let oracle = Oracle::parse(market, &ctx.dex_list("dex.propamm_price_slots", key).ok()?)?;
        let builders = builders(ctx.dex_list("dex.propamm_slot_builders", key).ok()?);
        if builders.is_empty() {
            return None;
        }
        Some(Self {
            family,
            oracle,
            builders,
        })
    }
}

/// Список для ручной котировки возвращает те же имена строителей и в том же порядке, что и допуск запроса.
pub(in crate::venues::bsc_dex) fn sources(ctx: &Ctx) -> BTreeMap<String, Vec<String>> {
    if network(ctx).is_none() {
        return BTreeMap::new();
    }
    families::all(ctx)
        .into_iter()
        .filter(|f| f.variant == Variant::PropVenue)
        .filter_map(|f| {
            let profile = Profile::read(ctx, f.id)?;
            let id = f.id.split('@').next()?;
            Some((
                id.to_owned(),
                profile
                    .builders
                    .into_iter()
                    .map(|b| b.name().to_owned())
                    .collect(),
            ))
        })
        .collect()
}
