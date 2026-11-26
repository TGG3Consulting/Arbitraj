//! Файл: venues/bsc_dex/pools/direct_module.rs.
//! Что делает: связывает адрес рынка и проверенный runtime отдельного исполнителя с протоколом и сетью.
//! Кто вызывает: families, exec и quote. От чего зависит: закреплённые архивные прогоны в first_run_dex.json.
//! Настройки: dex.kind и dex.router.
use super::super::{ctx::Ctx, node::rpc};
use super::{elfomo, families::Family, registry::Entry};
use crate::core::types::PoolRef;
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy_primitives::{Address, B256, Bytes, keccak256};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::OnceLock};

type Releases = BTreeMap<u64, BTreeMap<String, Vec<B256>>>;

fn releases(chain: u64, kind: &str) -> Option<&'static Vec<B256>> {
    static RELEASES: OnceLock<Releases> = OnceLock::new();
    RELEASES
        .get_or_init(|| {
            let mut out = Releases::new();
            let Ok(rows) = serde_json::from_str::<Vec<Value>>(include_str!(
                "../../../../config/first_run_dex.json"
            )) else {
                return out;
            };
            for r in rows {
                if let (Some(chain), Some(kind), Some(hash)) = (
                    r["chain_id"].as_u64(),
                    r["settings"]["dex.kind"].as_str(),
                    r["verified_module"]["runtime_keccak"]
                        .as_str()
                        .and_then(|s| s.parse().ok()),
                ) {
                    let mut versions = vec![hash];
                    if let Some(more) = r["verified_module"]["additional"].as_array() {
                        versions
                            .extend(more.iter().filter_map(|x| {
                                x["runtime_keccak"].as_str()?.parse::<B256>().ok()
                            }));
                    }
                    out.entry(chain)
                        .or_default()
                        .insert(kind.to_owned(), versions);
                }
            }
            out
        })
        .get(&chain)?
        .get(kind)
}

pub fn accepted(chain: u64, kind: &str) -> Option<B256> {
    releases(chain, kind)?.first().copied()
}

pub fn accepts(chain: u64, kind: &str, hash: B256) -> bool {
    releases(chain, kind).is_some_and(|xs| xs.contains(&hash))
}

pub fn market(ctx: &Ctx, entry: &Entry) -> GatewayResult<Address> {
    match (&entry.pool.reference, entry.family.proto) {
        (PoolRef::Member { contract, .. }, super::families::Proto::Elfomo)
            if *contract == elfomo::ROUTER =>
        {
            Ok(*contract)
        }
        (
            PoolRef::Member { contract, .. } | PoolRef::Contract(contract),
            super::families::Proto::BaiBai,
        ) if *contract == super::baibai::ENTRY => Ok(*contract),
        (PoolRef::Contract(address), super::families::Proto::Lunar)
            if *address == super::lunar::POOL =>
        {
            Ok(*address)
        }
        (PoolRef::Contract(address), super::families::Proto::PancakeV3)
            if entry.family.variant == super::family_variant::Variant::BaseNativeV3 =>
        {
            Ok(*address)
        }
        (PoolRef::Contract(address), super::families::Proto::Slipstream) => Ok(*address),
        _ => Err(ctx.err(
            ErrorCode::NoRoute,
            "Нет подтверждённого рынка прямого модуля",
        )),
    }
}

pub async fn verify_code(ctx: &Ctx, kind: &str, addr: Address, tag: &Value) -> GatewayResult<()> {
    let expected = ctx
        .chain_id()
        .and_then(|chain| releases(chain, kind))
        .ok_or_else(|| {
            ctx.err(
                ErrorCode::NotApplicable,
                format!("{kind}: отсутствует архивная приёмка модуля для сети"),
            )
        })?;
    let code: Bytes = rpc::call(ctx, "eth_getCode", json!([addr, tag])).await?;
    if !expected.contains(&keccak256(code)) {
        return Err(ctx.err(
            ErrorCode::InvalidAddress,
            format!("{kind}: код модуля расходится с принятой сборкой"),
        ));
    }
    Ok(())
}

pub async fn verify(ctx: &Ctx, f: &Family, addr: Address, at: &str) -> GatewayResult<()> {
    if f.variant == super::family_variant::Variant::BaseNativeV3 {
        if !super::base_native_v3::valid(ctx, f) {
            return Err(ctx.err(
                ErrorCode::NoRoute,
                "Изменена фабрика или квотер прямого V3-модуля Base",
            ));
        }
        return verify_code(ctx, "base_native_v3", addr, &json!(at)).await;
    }
    if f.proto == super::families::Proto::BaiBai {
        let tag = elfomo::block(ctx, at).await?;
        super::baibai::verify_at(ctx, f.id, &tag).await?;
        return verify_code(ctx, "baibai", addr, &tag).await;
    }
    if f.proto == super::families::Proto::Lunar {
        let tag = elfomo::block(ctx, at).await?;
        super::lunar::verify_at(ctx, f.id, &tag).await?;
        if !super::lunar::open(ctx, &tag, super::super::wallet::signer::address(ctx)?).await? {
            return Err(ctx.err(
                ErrorCode::NoRoute,
                "LunarBase на паузе или срок цены оператора истёк",
            ));
        }
        return verify_code(ctx, "lunarbase", addr, &tag).await;
    }
    if f.proto == super::families::Proto::Elfomo {
        let tag = elfomo::block(ctx, at).await?;
        elfomo::verify_at(ctx, f.id, elfomo::ROUTER, &tag).await?;
        return verify_code(ctx, "elfomofi", addr, &tag).await;
    }
    verify_code(ctx, "solidly_v3", addr, &json!(at)).await
}
