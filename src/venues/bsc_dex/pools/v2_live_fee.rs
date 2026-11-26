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

fn profile(chain: u64, factory: Address) -> Option<&'static serde_json::Value> {
    Some(
        &super::deployment_profile::for_factory(
            chain,
            factory,
            "v2_fork_executor_create2_live_fee",
        )?["settings"],
    )
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

async fn parent(ctx: &Ctx, tag: &str) -> GatewayResult<alloy_primitives::B256> {
    use super::super::node::rpc;
    #[derive(serde::Deserialize)]
    struct Header {
        number: alloy_primitives::U64,
        hash: alloy_primitives::B256,
    }
    let h: Header = rpc::call(ctx, "eth_getBlockByNumber", serde_json::json!([tag, false])).await?;
    if h.hash.is_zero() || format!("{:#x}", h.number) != tag {
        return Err(miss("Узел вернул другой блок для комиссии V2"));
    }
    Ok(h.hash)
}

type ReadCall = (Address, Bytes);
#[derive(Default)]
struct ReadPlan {
    calls: Vec<ReadCall>,
    indexes: std::collections::BTreeMap<ReadCall, usize>,
}

impl ReadPlan {
    fn index(&mut self, call: ReadCall) -> usize {
        *self.indexes.entry(call.clone()).or_insert_with(|| {
            let i = self.calls.len();
            self.calls.push(call);
            i
        })
    }
}

async fn read_chunk(
    ctx: &Ctx,
    calls: &[ReadCall],
    hash: alloy_primitives::B256,
) -> GatewayResult<Vec<Result<Bytes, String>>> {
    use super::super::node::{jsonrpc, multicall, rpc};
    use serde_json::json;
    let block = json!({"blockHash":hash,"requireCanonical":true});
    if let Some(mc) = multicall::address(ctx) {
        let calldata = multicall::encode(calls);
        let reply: Bytes =
            rpc::call(ctx, "eth_call", json!([{"to":mc,"data":calldata},block])).await?;
        return multicall::decode(&reply, calls.len());
    }
    let req: Vec<_> = calls
        .iter()
        .map(|(to, d)| ("eth_call", json!([{"to":to,"data":d},block])))
        .collect();
    Ok(rpc::batch(ctx, &req)
        .await?
        .into_iter()
        .map(|v| {
            v.and_then(|r| jsonrpc::decode(&r, rpc::SOURCE))
                .map_err(|e| e.message)
        })
        .collect())
}

pub(super) async fn many(
    ctx: &Ctx,
    pools: &[(usize, &Family, Address)],
    tag: &str,
    size: usize,
) -> Vec<GatewayResult<Loaded>> {
    let failure =
        |e: crate::gateway::errors::GatewayError| pools.iter().map(|_| Err(e.clone())).collect();
    let hash = match parent(ctx, tag).await {
        Ok(hash) => hash,
        Err(e) => return failure(e),
    };
    let mut reads = ReadPlan::default();
    let plans = pools
        .iter()
        .map(|(_, f, a)| {
            let rate = Rate::configured(ctx, f, *a)?;
            let ri = reads.index((*a, Bytes::from(IV2Pair::getReservesCall {}.abi_encode())));
            let fi = reads.index(rate.call());
            Ok((ri, fi, rate))
        })
        .collect::<Vec<GatewayResult<_>>>();
    let mut answers = Vec::with_capacity(reads.calls.len());
    for chunk in reads.calls.chunks(size.max(1)) {
        match read_chunk(ctx, chunk, hash).await {
            Ok(v) if v.len() == chunk.len() => answers.extend(v),
            Ok(_) => answers.extend(chunk.iter().map(|_| Err("Неполная пачка V2".to_owned()))),
            Err(e) => answers.extend(chunk.iter().map(|_| Err(e.message.clone()))),
        }
    }
    match parent(ctx, tag).await {
        Ok(current) if current == hash => {}
        Ok(_) => return failure(miss("Реорганизация во время чтения резервов и комиссии V2")),
        Err(e) => return failure(e),
    }
    plans
        .into_iter()
        .map(|plan| {
            let (ri, fi, rate) = plan?;
            let reserves = call::decode::<IV2Pair::getReservesCall>(
                answers[ri].as_ref().map_err(|m| miss(m.clone()))?,
            )?;
            let fee = rate.decode(answers[fi].as_ref().map_err(|m| miss(m.clone()))?)?;
            Ok(Loaded {
                state: PoolState::V2 {
                    reserve0: U256::from(reserves.reserve0),
                    reserve1: U256::from(reserves.reserve1),
                },
                ticks: None,
                bins: None,
                fee_pips: Some(fee),
            })
        })
        .collect()
}

pub(super) async fn load(ctx: &Ctx, f: &Family, pool: Address, tag: &str) -> GatewayResult<Loaded> {
    many(ctx, &[(0, f, pool)], tag, 2)
        .await
        .pop()
        .ok_or_else(|| miss("Нет результата чтения V2"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_factory_fee_is_read_once_and_other_targets_keep_their_own_indexes() {
        let factory = Address::repeat_byte(1);
        let first = Address::repeat_byte(2);
        let second = Address::repeat_byte(3);
        let fee = Bytes::from_static(&[1, 2, 3, 4]);
        let reserve = Bytes::from(IV2Pair::getReservesCall {}.abi_encode());
        let mut plan = ReadPlan::default();
        assert_eq!(plan.index((first, reserve.clone())), 0);
        assert_eq!(plan.index((factory, fee.clone())), 1);
        assert_eq!(plan.index((second, reserve)), 2);
        assert_eq!(plan.index((factory, fee.clone())), 1);
        assert_eq!(plan.index((first, fee)), 3);
        assert_eq!(plan.calls.len(), 4);
    }

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
