//! Проверенный пул iZiSwap с пятью делегированными реализациями. Адреса хранятся в последовательных слотах компоновки BitSwap.
use super::super::{ctx::Ctx, node::rpc};
use super::{
    abi_izi::{IIziFactory, IIziPool},
    families::Family,
    nami_batch as batch, registered_v3,
};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, aliases::U24};
use serde_json::{Value, json};
use std::collections::HashSet;
const FIRST_MODULE_SLOT: u64 = 65554;
fn bindings(lines: &[String]) -> Option<Vec<(Address, B256)>> {
    if lines.len() != 5 {
        return None;
    }
    let mut seen = HashSet::new();
    lines
        .iter()
        .map(|line| {
            let mut fields = line.split_whitespace();
            let a: Address = fields.next()?.parse().ok()?;
            let h: B256 = fields.next()?.parse().ok()?;
            (!a.is_zero() && !h.is_zero() && fields.next().is_none() && seen.insert(a))
                .then_some((a, h))
        })
        .collect()
}
fn matches(answers: &[Bytes], roles: &[(Address, B256)]) -> bool {
    answers.len() == roles.len()
        && answers
            .iter()
            .zip(roles)
            .all(|(slot, (a, _))| slot.len() == 32 && slot[12..] == a[..])
}
pub(in crate::venues::bsc_dex) async fn at(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    pair: (Address, Address),
    at: &Value,
) -> GatewayResult<()> {
    let generation = ctx.settings.generation();
    let bad = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "iZiSwap: код, делегаты или регистрация этой пары отличаются от профиля",
        )
    };
    if at["requireCanonical"] != true
        || at["blockHash"]
            .as_str()
            .and_then(|s| s.parse::<B256>().ok())
            .is_none()
        || pair.0 >= pair.1
    {
        return Err(bad());
    }
    let id = f.id;
    let factory = ctx.dex_addr("dex.factory", id)?;
    let allowed = bindings(&ctx.dex_list("dex.izi_module_bindings", id)?).ok_or_else(bad)?;
    let inits = ctx
        .dex_list("dex.pool_code_hashes", id)?
        .iter()
        .map(|s| s.parse::<B256>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| bad())?;
    let mut req: Vec<(&str, Value)> = Vec::new();
    for i in 0..allowed.len() {
        req.push((
            "eth_getStorageAt",
            json!([pool, format!("{:#x}", FIRST_MODULE_SLOT + i as u64), at]),
        ));
    }
    let roles = rpc::batch(ctx, &req);
    let fields = batch::read(
        ctx,
        [
            batch::view(at, pool, IIziPool::factoryCall {}),
            batch::view(at, pool, IIziPool::tokenXCall {}),
            batch::view(at, pool, IIziPool::tokenYCall {}),
            batch::view(at, pool, IIziPool::feeCall {}),
        ],
    );
    let checked = futures_util::future::try_join_all(
        allowed
            .iter()
            .map(|(a, h)| super::solidly_classic_factory::bind(ctx, *a, *h, None, at)),
    );
    let (_, _, roles, [fk, x, y, fee]) = tokio::try_join!(
        checked,
        super::v2_pair_binding::code(ctx, id, pool, at),
        roles,
        fields
    )?;
    let fee: U24 = batch::decode(&fee, IIziPool::feeCall {})?;
    if batch::decode(&fk, IIziPool::factoryCall {})? != factory
        || batch::decode(&x, IIziPool::tokenXCall {})? != pair.0
        || batch::decode(&y, IIziPool::tokenYCall {})? != pair.1
        || !inits
            .iter()
            .any(|init| registered_v3::address(factory, pair, fee, *init) == pool)
    {
        return Err(bad());
    }
    let bytes = roles
        .into_iter()
        .map(|item| serde_json::from_str::<Bytes>(item?.get()).map_err(|_| bad()))
        .collect::<GatewayResult<Vec<_>>>()?;
    if !matches(&bytes, &allowed) {
        return Err(bad());
    }
    let entry = |x, y| IIziFactory::poolCall {
        tokenX: x,
        tokenY: y,
        fee,
    };
    let [forward, reverse] = batch::read(
        ctx,
        [
            batch::view(at, factory, entry(pair.0, pair.1)),
            batch::view(at, factory, entry(pair.1, pair.0)),
        ],
    )
    .await?;
    if IIziFactory::poolCall::abi_decode_returns_validate(&forward).ok() != Some(pool)
        || IIziFactory::poolCall::abi_decode_returns_validate(&reverse).ok() != Some(pool)
        || generation != ctx.settings.generation()
    {
        return Err(bad());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_roles_must_bind_the_original_code_in_the_original_order() {
        let roles: Vec<_> = (1..=5)
            .map(|i| (Address::repeat_byte(i), alloy_primitives::keccak256([i])))
            .collect();
        let mut result: Vec<_> = roles
            .iter()
            .map(|(a, _)| Bytes::copy_from_slice(a.into_word().as_slice()))
            .collect();
        // flashModule shares this storage word with feeChargePercent (offset 20), as compiled in iZiSwapPool.
        let mut packed = result[4].to_vec();
        packed[11] = 50;
        result[4] = Bytes::from(packed);
        assert!(matches(&result, &roles));
        result.swap(0, 1);
        assert!(!matches(&result, &roles));
        result.swap(0, 1);
        result.pop();
        assert!(!matches(&result, &roles));
        assert!(bindings(&[]).is_none());
        let mut params: Vec<_> = roles.iter().map(|(a, h)| format!("{a} {h}")).collect();
        assert_eq!(bindings(&params), Some(roles));
        params.swap_remove(0);
        params.push(params[0].clone());
        assert!(bindings(&params).is_none());
    }
    #[test]
    fn all_nineteen_archived_pairs_derive_from_their_immutable_fee() {
        let j: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../tests/data/first_run_dex_2026-11-26/bitswap_ethereum_registry.json"
        ))
        .unwrap();
        let addr = |v: &Value| v.as_str().unwrap().parse::<Address>().unwrap();
        let rows = j["markets"].as_array().unwrap();
        assert_eq!(rows.len(), 19);
        for m in rows {
            let pair = (addr(&m["tokens"][0]), addr(&m["tokens"][1]));
            let fee = U24::from(m["fee"].as_u64().unwrap());
            let init = j["init"].as_str().unwrap().parse().unwrap();
            let factory = addr(&j["factory"]);
            assert_eq!(
                registered_v3::address(factory, pair, fee, init),
                addr(&m["pool"])
            );
            assert_ne!(
                registered_v3::address(factory, pair, fee + U24::ONE, init),
                addr(&m["pool"])
            );
        }
    }
}
