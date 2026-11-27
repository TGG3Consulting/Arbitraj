//! Ручная проверка канала чтением /prices из официального SimpleSwap API. Вызывается из venue.rs.
use super::{client, fail};
use crate::{
    gateway::{contract::ConnectionCheck, errors::GatewayResult},
    settings::evm_profiles,
    venues::bsc_dex::ctx::Ctx,
};
use serde::Deserialize;
use std::collections::BTreeMap;

pub(super) const PRICES: &str = "prices";

/// Формат раздела 3b SimpleSwap.md. Книга может быть пустой у зарегистрированного канала.
#[derive(Deserialize)]
pub(super) struct ChannelPrices {
    #[serde(rename = "prices")]
    _prices: BTreeMap<String, Levels>,
}

#[derive(Deserialize)]
struct Levels {
    #[serde(rename = "bids")]
    _bids: Vec<[String; 2]>,
    #[serde(rename = "asks")]
    _asks: Vec<[String; 2]>,
}

/// Доступ проверяется по читающему endpoint для физической сети Base, без firm и без подписи.
pub(in crate::venues::bsc_dex) async fn check_key(ctx: &Ctx) -> GatewayResult<ConnectionCheck> {
    let chain = ctx
        .chain_id()
        .and_then(evm_profiles::by_chain_id)
        .filter(|p| p.venue == "base_dex")
        .ok_or_else(|| fail(ctx, "Dexalot: проверка ключа доступна в сети Base"))?;
    let _: ChannelPrices = client::request(
        ctx,
        PRICES,
        &[("chainid", chain.evm_chain_id.to_string())],
        None,
    )
    .await?;
    Ok(ConnectionCheck {
        ok: true,
        details: "Dexalot принял ключ канала для чтения цен".into(),
    })
}
