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
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};

/// Подтверждение из работающего справочника настроек связывает формулу с сетью и кодом CREATE2.
fn rule_hash(
    chain: u64,
    factory: Address,
    init: B256,
    source: &str,
    getter: &str,
    denominator: u64,
) -> Option<B256> {
    let target = match source {
        "factory" => 1u8,
        "pool" => 0u8,
        _ => return None,
    };
    if factory.is_zero() || init.is_zero() || denominator == 0 {
        return None;
    }
    let digest = keccak256(getter.as_bytes());
    let mut rule = Vec::with_capacity(101);
    rule.extend_from_slice(&chain.to_be_bytes());
    rule.extend_from_slice(factory.as_slice());
    rule.extend_from_slice(init.as_slice());
    rule.push(target);
    rule.extend_from_slice(digest.as_slice());
    rule.extend_from_slice(&denominator.to_be_bytes());
    Some(keccak256(rule))
}

fn fee_call(getter: &str, pool: Address) -> Option<Bytes> {
    let (name, params) = getter.split_once('(')?;
    let mut letters = name.bytes();
    if !letters.next()?.is_ascii_alphabetic()
        || !letters.all(|c| c.is_ascii_alphanumeric() || c == b'_')
    {
        return None;
    }
    let selector = registry_key::selector(getter);
    match params {
        ")" => Some(selector),
        "address)" => {
            let mut call = Vec::with_capacity(36);
            call.extend_from_slice(&selector);
            call.extend_from_slice(&[0u8; 12]);
            call.extend_from_slice(pool.as_slice());
            Some(call.into())
        }
        _ => None,
    }
}

pub(in crate::venues::bsc_dex) struct Rate {
    target: Address,
    selector: Bytes,
    denominator: u64,
}

struct FeeRule {
    factory: Option<Address>,
    getter: String,
    denominator: u64,
}
impl FeeRule {
    fn read(ctx: &Ctx, family: &Family) -> GatewayResult<Self> {
        let factory = ctx.dex_addr("dex.factory", family.id)?;
        let source = ctx.dex_text("dex.v2_fee_source", family.id)?;
        let getter = ctx.dex_text("dex.v2_fee_method", family.id)?;
        let denominator = ctx.dex_int("dex.v2_fee_denominator", family.id)?;
        let hashes = ctx.dex_list("dex.pool_code_hashes", family.id)?;
        let configured = ctx
            .dex_text("dex.v2_fee_rule_hash", family.id)?
            .parse::<B256>()
            .map_err(|_| miss("dex.v2_fee_rule_hash не является keccak256"))?;
        let actual = match (ctx.chain_id(), hashes.as_slice()) {
            (Some(chain), [init]) => init
                .parse::<B256>()
                .ok()
                .and_then(|h| rule_hash(chain, factory, h, &source, &getter, denominator)),
            _ => None,
        };
        if actual != Some(configured) {
            return Err(miss(
                "Сеть, фабрика, хеш создания пары или формула комиссии отличаются от подтверждения в dex.v2_fee_rule_hash",
            ));
        }
        Ok(Self {
            factory: (source == "factory").then_some(factory),
            getter,
            denominator,
        })
    }
    fn at(&self, pool: Address) -> GatewayResult<Rate> {
        Ok(Rate {
            target: self.factory.unwrap_or(pool),
            selector: fee_call(&self.getter, pool).ok_or_else(|| {
                miss("Метод комиссии должен принимать адрес пары или не иметь аргументов")
            })?,
            denominator: self.denominator,
        })
    }
}
impl Rate {
    pub fn configured(ctx: &Ctx, family: &Family, pool: Address) -> GatewayResult<Self> {
        FeeRule::read(ctx, family)?.at(pool)
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
    let mut rules = std::collections::BTreeMap::new();
    let plans = pools
        .iter()
        .map(|(_, f, a)| {
            let rule = rules
                .entry(f.id)
                .or_insert_with(|| FeeRule::read(ctx, f))
                .as_ref()
                .map_err(Clone::clone)?;
            let rate = rule.at(*a)?;
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
            let getter = r["fee_getter"].as_str().unwrap();
            let den = r["fee_fraction"][0].as_u64().unwrap();
            let init = r["creation_code_hash"].as_str().unwrap().parse().unwrap();
            let (source, wrong) = if r["fee_target"] == r["factory"] {
                ("factory", "pool")
            } else {
                ("pool", "factory")
            };
            let rows: Vec<serde_json::Value> =
                serde_json::from_str(include_str!("../../../../config/first_run_dex.json"))
                    .unwrap();
            let id = r["dex"].as_str().unwrap();
            let spec = rows
                .iter()
                .find(|v| v["chain_id"] == chain && v["id"] == id)
                .unwrap();
            let binding = Some(
                spec["settings"]["dex.v2_fee_rule_hash"]
                    .as_str()
                    .unwrap()
                    .parse()
                    .unwrap(),
            );
            assert_eq!(
                rule_hash(chain, factory, init, source, getter, den),
                binding
            );
            assert_ne!(rule_hash(chain, factory, init, wrong, getter, den), binding);
            assert_ne!(
                rule_hash(chain, factory, init, source, "decimals()", den),
                binding
            );
            assert_ne!(
                rule_hash(chain, factory, init, source, getter, den + 1),
                binding
            );
            assert_ne!(rule_hash(56, factory, init, source, getter, den), binding);
        }
    }
}
