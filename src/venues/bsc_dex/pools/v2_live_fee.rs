//! Комиссия CREATE2-пар, чей swap читает изменяемую ставку у пары или фабрики.
//! Пара getReserves/getter читается на одном блоке при допуске, восстановлении и перед обменом.
//! Настройки: dex.v2_fee_source, dex.v2_fee_method, dex.v2_fee_denominator, dex.factory.
use super::{
    abi_v2v3::IV2Pair, families::Family, load::Loaded, load_batch::miss, registry_key, verify_rules,
};
use crate::core::types::PoolState;
use crate::gateway::errors::GatewayResult;
use crate::venues::bsc_dex::{ctx::Ctx, node::call};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

#[derive(serde::Deserialize)]
struct Profile {
    settings: serde_json::Value,
    chain_id: u64,
}

fn profile(chain: u64, factory: Address) -> Option<&'static serde_json::Value> {
    use std::{collections::BTreeMap, sync::OnceLock};
    static PROFILES: OnceLock<BTreeMap<(u64, Address), serde_json::Value>> = OnceLock::new();
    PROFILES
        .get_or_init(|| {
            let rows: Vec<Profile> =
                serde_json::from_str(include_str!("../../../../config/first_run_dex.json"))
                    .expect("встроенные профили DEX");
            rows.into_iter()
                .filter_map(|r| {
                    if r.settings["dex.kind"] != "v2_fork_executor_create2_live_fee" {
                        return None;
                    }
                    let factory = r.settings["dex.factory"].as_str()?.parse().ok()?;
                    Some(((r.chain_id, factory), r.settings))
                })
                .collect()
        })
        .get(&(chain, factory))
}

fn matches_rate(
    expected: &serde_json::Value,
    source: &str,
    getter: &str,
    denominator: u64,
    hashes: &[String],
) -> bool {
    expected["dex.v2_fee_source"].as_str() == Some(source)
        && expected["dex.v2_fee_method"].as_str() == Some(getter)
        && expected["dex.v2_fee_denominator"].as_u64() == Some(denominator)
        && hashes.len() == 1
        && expected["dex.pool_code_hashes"][0]
            .as_str()
            .is_some_and(|h| h.eq_ignore_ascii_case(hashes[0].trim()))
}

pub(in crate::venues::bsc_dex) struct Rate {
    target: Address,
    selector: Bytes,
    denominator: u64,
}

impl Rate {
    pub fn configured(ctx: &Ctx, family: &Family, pool: Address) -> GatewayResult<Self> {
        let factory = ctx.dex_addr("dex.factory", family.id)?;
        let expected = ctx
            .chain_id()
            .and_then(|id| profile(id, factory))
            .ok_or_else(|| {
                miss("Нет проверенного профиля переменной комиссии V2 для сети и фабрики")
            })?;
        let source = ctx.dex_text("dex.v2_fee_source", family.id)?;
        let getter = ctx.dex_text("dex.v2_fee_method", family.id)?;
        let denominator = ctx.dex_int("dex.v2_fee_denominator", family.id)?;
        let hashes = ctx.dex_list("dex.pool_code_hashes", family.id)?;
        if !matches_rate(expected, &source, &getter, denominator, &hashes) {
            return Err(miss(
                "Параметры комиссии V2 расходятся с проверенным профилем фабрики",
            ));
        }
        let target = if source == "factory" { factory } else { pool };
        Ok(Self {
            target,
            selector: registry_key::selector(&getter),
            denominator,
        })
    }

    pub fn call(&self) -> (Address, Bytes) {
        (self.target, self.selector.clone())
    }

    pub fn decode(&self, bytes: &[u8]) -> GatewayResult<u32> {
        if bytes.len() != 32 {
            return Err(miss("Ответ метода комиссии V2 не равен одному слову ABI"));
        }
        verify_rules::v2_fee_pips(U256::from_be_slice(bytes), self.denominator).map_err(miss)
    }
}

pub(super) async fn load(
    ctx: &Ctx,
    family: &Family,
    pool: Address,
    tag: &str,
) -> GatewayResult<Loaded> {
    let rate = Rate::configured(ctx, family, pool)?;
    let calls = [
        (pool, Bytes::from(IV2Pair::getReservesCall {}.abi_encode())),
        rate.call(),
    ];
    let values = call::try_many(ctx, &calls, tag).await?;
    let [reserves, fee] = values.as_slice() else {
        return Err(miss("Неполный ответ о резервах и комиссии V2"));
    };
    let unpack = |v: &Result<Bytes, String>| v.clone().map_err(miss);
    let reserves = call::decode::<IV2Pair::getReservesCall>(&unpack(reserves)?)?;
    Ok(Loaded {
        state: PoolState::V2 {
            reserve0: U256::from(reserves.reserve0),
            reserve1: U256::from(reserves.reserve1),
        },
        ticks: None,
        bins: None,
        fee_pips: Some(rate.decode(&unpack(fee)?)?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn verified_fee_origin_cannot_be_replaced_by_an_unrelated_successful_view() {
        let rows: Vec<serde_json::Value> = serde_json::from_str(include_str!(
            "../../../../tests/data/base_ethereum_v2_variable_fees.json"
        ))
        .unwrap();
        for r in rows {
            let chain = if r["network"] == "base" { 8453 } else { 1 };
            let factory = r["factory"].as_str().unwrap().parse().unwrap();
            let spec = profile(chain, factory).unwrap();
            let getter = r["fee_getter"].as_str().unwrap();
            let den = r["fee_fraction"][0].as_u64().unwrap();
            let hash = [r["creation_code_hash"].as_str().unwrap().to_owned()];
            let factory_fee = r["fee_target"] == r["factory"];
            let (source, wrong) = if factory_fee {
                ("factory", "pool")
            } else {
                ("pool", "factory")
            };
            assert!(matches_rate(spec, source, getter, den, &hash));
            assert!(!matches_rate(spec, wrong, getter, den, &hash));
            assert!(!matches_rate(spec, source, "decimals()", den, &hash));
            assert!(!matches_rate(spec, source, getter, den + 1, &hash));
            assert!(!matches_rate(spec, source, getter, den, &[]));
            assert!(profile(56, factory).is_none());
        }
    }
}
