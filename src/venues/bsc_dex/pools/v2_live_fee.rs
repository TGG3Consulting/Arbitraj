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
        "pool_retained" => 2u8,
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

#[derive(Clone, Copy)]
pub(in crate::venues::bsc_dex) struct Market {
    pub pool: Address,
    pub tokens: (Address, Address),
}

fn fee_call(getter: &str, market: Market) -> Option<Bytes> {
    use alloy::sol_types::SolValue;

    let (name, params) = getter.split_once('(')?;
    let mut letters = name.bytes();
    if !letters.next()?.is_ascii_alphabetic()
        || !letters.all(|c| c.is_ascii_alphanumeric() || c == b'_')
    {
        return None;
    }
    let selector = registry_key::selector(getter);
    let args = match params {
        ")" => return Some(selector),
        "address)" => market.pool.abi_encode(),
        "address,address)" => market.tokens.abi_encode_params(),
        _ => return None,
    };
    let mut call = selector.to_vec();
    call.extend_from_slice(&args);
    Some(call.into())
}

pub(in crate::venues::bsc_dex) struct Rate {
    target: Address,
    selector: Bytes,
    denominator: u64,
    retained: bool,
    mars_info: bool,
}

struct FeeRule {
    factory: Address,
    factory_source: bool,
    retained: bool,
    init: B256,
    getter: String,
    denominator: u64,
}
impl FeeRule {
    fn read(ctx: &Ctx, family: &Family) -> GatewayResult<Self> {
        let factory = ctx.dex_addr("dex.factory", family.id)?;
        let source = ctx.dex_text("dex.v2_fee_source", family.id)?;
        let getter = ctx.dex_text("dex.v2_fee_method", family.id)?;
        let denominator = ctx.dex_int("dex.v2_fee_denominator", family.id)?;
        if family.variant == super::family_variant::Variant::DiamondV2
            && (source!="factory" || getter!="swapFee()" || denominator!=10_000) {
            return Err(miss("Вариант Diamond V2 требует swapFee() фабрики над 10000"));
        }
        if family.variant == super::family_variant::Variant::CandySwap
            && (source!="pool" || getter!="swapFee()" || denominator!=10_000) {
            return Err(miss("CandySwap: требуется swapFee() пары и знаменатель 10000"));
        }
        let hashes = ctx.dex_list("dex.pool_code_hashes", family.id)?;
        let configured = ctx
            .dex_text("dex.v2_fee_rule_hash", family.id)?
            .parse::<B256>()
            .map_err(|_| miss("dex.v2_fee_rule_hash не является keccak256"))?;
        let init = match hashes.as_slice() {
            [init] => init.parse::<B256>().ok(),
            _ => None,
        }
        .ok_or_else(|| miss("Требуется один хеш создания пары V2"))?;
        let actual = ctx
            .chain_id()
            .and_then(|chain| rule_hash(chain, factory, init, &source, &getter, denominator));
        if actual != Some(configured) {
            return Err(miss(
                "Сеть, фабрика, хеш создания пары или формула комиссии отличаются от подтверждения в dex.v2_fee_rule_hash",
            ));
        }
        Ok(Self {
            factory,
            factory_source: source == "factory",
            retained: source == "pool_retained",
            init,
            getter,
            denominator,
        })
    }
    fn at(&self, market: Market) -> GatewayResult<Rate> {
        let (a, b) = market.tokens;
        if a.is_zero()
            || a >= b
            || super::verify::v2_create2(self.factory, a, b, self.init) != market.pool
        {
            return Err(miss(
                "Адрес пары не совпадает с CREATE2 текущих настроек фабрики V2",
            ));
        }
        Ok(Rate {
            target: if self.factory_source { self.factory } else { market.pool },
            selector: fee_call(&self.getter, market).ok_or_else(|| {
                miss("Метод комиссии должен принимать (), (address пары) или (address token0, address token1)")
            })?,
            denominator: self.denominator,
            retained: self.retained,
            mars_info: self.factory_source && self.getter == "lpInfo()",
        })
    }
}
impl Rate {
    pub fn configured(ctx: &Ctx, family: &Family, market: Market) -> GatewayResult<Self> {
        FeeRule::read(ctx, family)?.at(market)
    }

    pub fn call(&self) -> (Address, Bytes) {
        (self.target, self.selector.clone())
    }

    pub fn decode(&self, bytes: &[u8]) -> GatewayResult<u32> {
        let value = if self.mars_info {
            mars_percentage(bytes, self.denominator)?
        } else {
            if bytes.len() != 32 {
                return Err(miss("Ответ метода комиссии V2 не равен одному слову ABI"));
            }
            U256::from_be_slice(bytes)
        };
        let value = if self.retained {
            U256::from(self.denominator).checked_sub(value)
                .ok_or_else(|| miss("Сохраняемая доля V2 превышает знаменатель"))?
        } else { value };
        if value >= U256::from(self.denominator) {
            return Err(miss("Комиссия V2 должна быть меньше 100%"));
        }
        if value * U256::from(1_000_000) % U256::from(self.denominator) != U256::ZERO {
            return Err(miss(
                "Комиссия V2 не представима целым числом миллионных долей",
            ));
        }
        verify_rules::v2_fee_pips(value, self.denominator).map_err(miss)
    }
}

/// MarSwap Router02 и V5 при нулевом msg.value. lpInfo() отдаёт treasury, LP bps,
/// Mars bps и стоимость фиксированной оплаты в wei. Адрес treasury не является ставкой.
fn mars_percentage(bytes: &[u8], denominator: u64) -> GatewayResult<U256> {
    if denominator != 10_000 || bytes.len() != 128 || bytes[..12] != [0;12] {
        return Err(miss("MarSwap: неверная структура lpInfo() или знаменатель"));
    }
    let lp = U256::from_be_slice(&bytes[32..64]);
    let platform = U256::from_be_slice(&bytes[64..96]);
    let total = lp.checked_add(platform).filter(|x| *x < U256::from(denominator))
        .ok_or_else(|| miss("MarSwap: недопустимая сумма двух ставок"))?;
    if bytes[96..].iter().all(|x| *x == 0) {
        return Err(miss("MarSwap: льготный режим с нулевой фиксированной платой требует отдельной проверки пары"));
    }
    Ok(total)
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
    pools: &[(usize, &Family, Market)],
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
    for (_,f,_) in pools {
        if f.variant==super::family_variant::Variant::DiamondV2 && let std::collections::btree_map::Entry::Vacant(slot)=rules.entry(f.id) {
            let at=serde_json::json!({"blockHash":hash,"requireCanonical":true});
            let result=match super::diamond::check(ctx,f.id,false,&at).await {
                Ok(())=>FeeRule::read(ctx,f),
                Err(error)=>Err(error),
            };
            slot.insert(result);
        }
    }
    let plans = pools
        .iter()
        .map(|(_, f, a)| {
            let rule = rules
                .entry(f.id)
                .or_insert_with(|| FeeRule::read(ctx, f))
                .as_ref()
                .map_err(Clone::clone)?;
            let rate = rule.at(*a)?;
            let ri = reads.index((
                a.pool,
                Bytes::from(IV2Pair::getReservesCall {}.abi_encode()),
            ));
            let fi = reads.index(rate.call());
            let candy = (f.variant == super::family_variant::Variant::CandySwap).then(|| {
                use super::super::node::abi::IERC20;
                let balance = Bytes::from(IERC20::balanceOfCall { owner: a.pool }.abi_encode());
                [
                    reads.index((a.pool, Bytes::from_static(&super::super::math::candy::PAYOUT_RATE_SELECTOR))),
                    reads.index((a.tokens.0, balance.clone())),
                    reads.index((a.tokens.1, balance)),
                    reads.index((rule.factory, registry_key::selector("feeTo()"))),
                ]
            });
            Ok((ri, fi, rate, candy))
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
            let (ri, fi, rate, candy) = plan?;
            let reserves = call::decode::<IV2Pair::getReservesCall>(
                answers[ri].as_ref().map_err(|m| miss(m.clone()))?,
            )?;
            let fee = rate.decode(answers[fi].as_ref().map_err(|m| miss(m.clone()))?)?;
            let reserve0 = U256::from(reserves.reserve0);
            let reserve1 = U256::from(reserves.reserve1);
            let state = if let Some([payout, b0, b1, collector]) = candy {
                let word = |index: usize| {
                    let bytes = answers[index].as_ref().map_err(|m| miss(m.clone()))?;
                    if bytes.len()!=32 {return Err(miss("CandySwap: ответ чтения не равен слову ABI"));}
                    Ok(U256::from_be_slice(bytes))
                };
                let bps=word(payout)?;
                let collector=word(collector)?.to_be_bytes::<32>();
                if bps>U256::from(10_000) || collector[..12]!=[0;12] {
                    return Err(miss("CandySwap: недействительная ставка выплаты или получатель"));
                }
                PoolState::CandySwap(Box::new(crate::core::types::CandySwapState {
                    reserves:[reserve0,reserve1], balances:[word(b0)?,word(b1)?],
                    payout_bps:bps.to::<u16>(), receiver:Address::from_slice(&collector[12..]),
                }))
            } else {PoolState::V2 {reserve0,reserve1}};
            Ok(Loaded {
                state,
                ticks: None,
                bins: None,
                fee_pips: Some(fee),
            })
        })
        .collect()
}

pub(super) async fn load(
    ctx: &Ctx,
    f: &Family,
    market: Market,
    tag: &str,
) -> GatewayResult<Loaded> {
    many(ctx, &[(0, f, market)], tag, 2)
        .await
        .pop()
        .ok_or_else(|| miss("Нет результата чтения V2"))?
}

/// Два изменяемых тарифа CandySwap должны соответствовать подписываемому маршруту.
/// Для других семейств эта проверка не вызывается.
pub(in crate::venues::bsc_dex) async fn candy_fresh(
    ctx:&Ctx,e:&super::registry::Entry,at:&serde_json::Value
)->GatewayResult<()> {
    use super::nami_batch;
    use serde_json::json;
    let bad=||miss("CandySwap: ставка пары, удержание выплаты или получатель изменились");
    let PoolState::CandySwap(state)=&e.pool.state else {return Err(bad());};
    let pool=e.pool.reference.own_contract().ok_or_else(bad)?;
    let factory=ctx.dex_addr("dex.factory",e.family.id)?;
    use super::super::node::abi::IERC20;
    let reserves=IV2Pair::getReservesCall{};
    let balance=IERC20::balanceOfCall{owner:pool};
    let [fee,rate,receiver,saved,physical0,physical1]=nami_batch::read(ctx,[
        ("eth_call",json!([{"to":pool,"data":registry_key::selector("swapFee()")},at])),
        ("eth_call",json!([{"to":pool,"data":"0xc3595a85"},at])),
        ("eth_call",json!([{"to":factory,"data":registry_key::selector("feeTo()")},at])),
        nami_batch::view(at,pool,reserves),
        nami_batch::view(at,e.pool.token0,balance.clone()),
        nami_batch::view(at,e.pool.token1,balance),
    ]).await?;
    let saved=call::decode::<IV2Pair::getReservesCall>(&saved)?;
    if [physical0.len(),physical1.len()]!=[32,32]{return Err(bad());}
    for (i,(actual,book)) in [(physical0,saved.reserve0),(physical1,saved.reserve1)].into_iter().enumerate() {
        if U256::from_be_slice(&actual).cmp(&U256::from(book))!=state.balances[i].cmp(&state.reserves[i]){
            return Err(miss("CandySwap: баланс относительно резерва изменился, требуется свежая котировка"));
        }
    }
    let pips=e.pool.fee_pips.ok_or_else(bad)?;
    if fee.len()!=32 || rate.len()!=32 || receiver.len()!=32
        || U256::from_be_slice(&fee).checked_mul(U256::from(100))!=Some(U256::from(pips))
        || U256::from_be_slice(&rate)!=U256::from(state.payout_bps)
        || receiver[..12]!=[0;12] || Address::from_slice(&receiver[12..])!=state.receiver {
        return Err(bad());
    }
    // Эти совпадения меняют прирост баланса конкретного получателя и повторное использование пары.
    if state.receiver==pool || state.receiver==super::super::wallet::signer::address(ctx)?
        || super::super::exec::executor::address(ctx)?==Some(state.receiver) {
        return Err(miss("CandySwap: адрес сбора совпал с одним из участников маршрута; требуется повторная сверка его баланса"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pair_and_tokens_fee_calls_encode_the_registered_market() {
        use alloy::sol_types::SolValue;
        let factory = Address::repeat_byte(1);
        let init = B256::repeat_byte(3);
        let tokens = (Address::repeat_byte(2), Address::repeat_byte(4));
        let pool = super::super::verify::v2_create2(factory, tokens.0, tokens.1, init);
        let market = Market { pool, tokens };
        let getter = "getFeeForPair(address,address)";
        let rule = FeeRule {
            factory,
            init,
            getter: getter.to_owned(),
            factory_source: true,
            retained: false,
            denominator: 10_000,
        };
        let rate = rule.at(market).unwrap();
        let (target, calldata) = rate.call();
        assert_eq!(target, factory);
        assert_eq!(&calldata[..4], registry_key::selector(getter).as_ref());
        assert_eq!(&calldata[4..], tokens.abi_encode_params());
        assert_eq!(rate.decode(&U256::from(25).abi_encode()).unwrap(), 2500);
        assert!(
            rule.at(Market {
                pool: factory,
                ..market
            })
            .is_err()
        );
        assert!(
            rule.at(Market {
                pool,
                tokens: (tokens.1, tokens.0)
            })
            .is_err()
        );
        assert_eq!(
            &fee_call("getFeeForPair(address)", market).unwrap()[4..],
            pool.abi_encode()
        );
        let retained = Rate { retained:true, ..rate };
        for (net, pips) in [(9970u64,3000),(9990,1000),(10000,0),(9700,30000)] {
            assert_eq!(retained.decode(&U256::from(net).abi_encode()).unwrap(),pips);
        }
        assert!(retained.decode(&U256::from(10001).abi_encode()).is_err());
        assert_ne!(rule_hash(1,factory,init,"pool",getter,10000),
                   rule_hash(1,factory,init,"pool_retained",getter,10000));
        let inexact = Rate {
            denominator: 3,
            retained: false,
            ..retained
        };
        assert!(inexact.decode(&U256::from(1).abi_encode()).is_err());
    }

    #[test]
    fn mars_terms_sum_only_the_variable_components() {
        use alloy::sol_types::SolValue;
        let t = |lp, fee, flat| (Address::repeat_byte(9), U256::from(lp), U256::from(fee), U256::from(flat)).abi_encode_params();
        assert_eq!(mars_percentage(&t(10u64, 10u64, 180_000_000_000_000u64), 10_000).unwrap(), U256::from(20));
        assert_eq!(mars_percentage(&t(999, 0, 1), 10_000).unwrap(), U256::from(999));
        assert!(mars_percentage(&t(9999, 1, 1), 10_000).is_err());
        assert!(mars_percentage(&t(10, 10, 0), 10_000).is_err());
        assert!(mars_percentage(&[0;32], 10_000).is_err());
    }

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
