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

pub fn accepted(chain: u64, kind: &str) -> Option<B256> {
    static ACCEPTED: OnceLock<BTreeMap<u64, BTreeMap<String, B256>>> = OnceLock::new();
    ACCEPTED
        .get_or_init(|| {
            let mut out: BTreeMap<u64, BTreeMap<String, B256>> = BTreeMap::new();
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
                    out.entry(chain).or_default().insert(kind.to_owned(), hash);
                }
            }
            out
        })
        .get(&chain)?
        .get(kind)
        .copied()
}

pub fn market(ctx: &Ctx, entry: &Entry) -> GatewayResult<Address> {
    match (&entry.pool.reference, entry.family.proto) {
        (PoolRef::Member { contract, .. }, super::families::Proto::Elfomo)
            if *contract == elfomo::ROUTER =>
        {
            Ok(*contract)
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
        .and_then(|chain| accepted(chain, kind))
        .ok_or_else(|| {
            ctx.err(
                ErrorCode::NotApplicable,
                format!("{kind}: отсутствует архивная приёмка модуля для сети"),
            )
        })?;
    let code: Bytes = rpc::call(ctx, "eth_getCode", json!([addr, tag])).await?;
    if keccak256(code) != expected {
        return Err(ctx.err(
            ErrorCode::InvalidAddress,
            format!("{kind}: код модуля расходится с принятой сборкой"),
        ));
    }
    Ok(())
}

pub async fn verify(ctx: &Ctx, f: &Family, addr: Address, at: &str) -> GatewayResult<()> {
    if f.proto == super::families::Proto::Elfomo {
        let tag = elfomo::block(ctx, at).await?;
        elfomo::verify_at(ctx, f.id, elfomo::ROUTER, &tag).await?;
        return verify_code(ctx, "elfomofi", addr, &tag).await;
    }
    verify_code(ctx, "solidly_v3", addr, &json!(at)).await
}
