//! Проверка адресов Ring на одном каноническом блоке Ethereum.
//! Каждый кандидат связывается с установленной фабрикой, точным кодом пула и
//! обеих обёрток, реестрами в обе стороны и decimals исходной монеты.
//! Сетевые обращения выполняются при приёме кандидата в реестр.

use super::{
    discover::Found,
    ring,
    verify::{Candidate, Verdict},
};
use crate::core::types::PoolRef;
use crate::venues::bsc_dex::{ctx::Ctx, node::rpc};
use alloy::sol;
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use serde_json::{Value, json};

sol! {
    interface IRingProof {
        function getWrappedToken(address asset) external view returns (address);
        function getPair(address a, address b) external view returns (address);
        function WETH() external view returns (address);
        function fwWETH() external view returns (address);
        function fewFactory() external view returns (address);
        function factory() external view returns (address);
        function token() external view returns (address);
        function token0() external view returns (address);
        function token1() external view returns (address);
        function decimals() external view returns (uint8);
        function balanceOf(address owner) external view returns (uint256);
    }
}

fn view<C: SolCall>(target: Address, method: C) -> (Address, Bytes) {
    (target, Bytes::from(method.abi_encode()))
}

async fn aggregate(
    ctx: &Ctx,
    multicall_address: Address,
    calls: &[(Address, Bytes)],
    tag: &Value,
) -> crate::gateway::errors::GatewayResult<Vec<Result<Bytes, String>>> {
    use crate::venues::bsc_dex::node::multicall;
    let data = multicall::encode(calls);
    let bytes: Bytes = rpc::call(
        ctx,
        "eth_call",
        json!([{"to":multicall_address,"input":data},tag]),
    )
    .await?;
    multicall::decode(&bytes, calls.len())
}

async fn code(ctx: &Ctx, a: Address, tag: &Value) -> crate::gateway::errors::GatewayResult<B256> {
    let raw: Bytes = rpc::call(ctx, "eth_getCode", json!([a, tag])).await?;
    Ok(keccak256(raw))
}

/// Solidity immutables: ищем целые операнды PUSH32, пропуская данные других PUSH.
fn router_links(code: &[u8], layout: &ring::Layout) -> bool {
    let expected =
        [layout.factory, layout.few, layout.permit, layout.native].map(|a| a.into_word());
    let mut found = [false; 4];
    let mut index = 0;
    while let Some(&op) = code.get(index) {
        index += 1;
        if (0x60..=0x7f).contains(&op) {
            let end = index + usize::from(op - 0x5f);
            let Some(value) = code.get(index..end) else {
                break;
            };
            if op == 0x7f {
                for (slot, key) in found.iter_mut().zip(&expected) {
                    *slot |= value == key.as_slice();
                }
            }
            index = end;
        }
    }
    found.into_iter().all(|x| x)
}

async fn infrastructure(ctx: &Ctx, layout: &ring::Layout, tag: &Value) -> Result<(), String> {
    let replies = futures_util::future::try_join_all(
        layout
            .contracts()
            .into_iter()
            .map(|a| rpc::call::<Bytes>(ctx, "eth_getCode", json!([a, tag]))),
    )
    .await
    .map_err(|e| e.message)?;
    let hashes_match = replies
        .iter()
        .zip(&layout.infrastructure)
        .all(|(code, hash)| !code.is_empty() && keccak256(code) == *hash);
    if hashes_match && router_links(&replies[3], layout) {
        Ok(())
    } else {
        Err("Ring: код контрактов или адреса в коде Universal Router расходятся".into())
    }
}

struct Market {
    layout: ring::Layout,
    pool: Address,
    assets: [Address; 2],
    fw: [Address; 2],
}

impl Market {
    fn from(c: &Candidate, layout: ring::Layout) -> Option<Self> {
        let pool = c.pool.parse().ok()?;
        let [a, b] = layout.path(c.coin, c.quote)?;
        if !layout.check_pool(pool, a, b) {
            return None;
        }
        let (assets, fw) = if a < b {
            ([c.coin, c.quote], [a, b])
        } else {
            ([c.quote, c.coin], [b, a])
        };
        Some(Self {
            layout,
            pool,
            assets,
            fw,
        })
    }
    fn requests(&self) -> Vec<(Address, Bytes)> {
        let mut calls = vec![
            view(self.layout.quoter, IRingProof::WETHCall {}),
            view(self.layout.quoter, IRingProof::fwWETHCall {}),
            view(self.layout.quoter, IRingProof::factoryCall {}),
            view(self.layout.quoter, IRingProof::fewFactoryCall {}),
            view(self.pool, IRingProof::factoryCall {}),
            view(self.pool, IRingProof::token0Call {}),
            view(self.pool, IRingProof::token1Call {}),
            view(
                self.layout.factory,
                IRingProof::getPairCall {
                    a: self.fw[0],
                    b: self.fw[1],
                },
            ),
        ];
        for (&asset, &fw) in self.assets.iter().zip(&self.fw) {
            calls.extend([
                view(fw, IRingProof::factoryCall {}),
                view(fw, IRingProof::tokenCall {}),
                view(self.layout.few, IRingProof::getWrappedTokenCall { asset }),
                view(fw, IRingProof::decimalsCall {}),
                view(asset, IRingProof::decimalsCall {}),
            ]);
        }
        calls
    }

    fn check(&self, rows: &[Bytes]) -> bool {
        let addr = |data: &Bytes, a: Address| data.as_ref() == a.into_word().as_slice();
        let [
            qnative,
            qfw,
            qfactory,
            qfew,
            factory,
            t0,
            t1,
            pair,
            wrappers @ ..,
        ] = rows
        else {
            return false;
        };
        if !addr(qnative, self.layout.native)
            || !addr(qfw, self.layout.wrapped(self.layout.native))
            || !addr(qfactory, self.layout.factory)
            || !addr(qfew, self.layout.few)
            || !addr(factory, self.layout.factory)
            || !addr(t0, self.fw[0])
            || !addr(t1, self.fw[1])
            || !addr(pair, self.pool)
            || wrappers.len() != 10
        {
            return false;
        }
        wrappers
            .chunks_exact(5)
            .zip(self.assets.iter().zip(&self.fw))
            .all(|(r, (&asset, &fw))| {
                addr(&r[0], self.layout.few)
                    && addr(&r[1], asset)
                    && addr(&r[2], fw)
                    && r[3].len() == 32
                    && r[3] == r[4]
                    && U256::from_be_slice(&r[3]) <= U256::from(77)
            })
    }

    async fn verdict(&self, ctx: &Ctx, c: &Candidate, tag: &Value, initial: (u64, u64)) -> Verdict {
        let calls = self.requests();
        let response = tokio::try_join!(
            code(ctx, self.pool, tag),
            code(ctx, self.fw[0], tag),
            code(ctx, self.fw[1], tag),
            aggregate(ctx, self.layout.multicall, &calls, tag)
        );
        let (pool, fw0, fw1, words) = match response {
            Ok(v) => v,
            Err(e) => return Verdict::Failed(e.message),
        };
        let Ok(words) = words.into_iter().collect::<Result<Vec<_>, _>>() else {
            return Verdict::Rejected("Ring: ошибка чтения связанных контрактов".into());
        };
        if pool != self.layout.pair_runtime
            || fw0 != self.layout.wrapper_runtime
            || fw1 != self.layout.wrapper_runtime
            || !self.check(&words)
        {
            return Verdict::Rejected(
                "Ring: код, реестр, исходный токен или decimals расходятся".into(),
            );
        }
        if ctx.settings.generation() != initial.0
            || ctx.int("dex.evm_chain_id").ok() != Some(initial.1)
        {
            return Verdict::Failed(
                "Ring: настройки изменились во время проверки, требуется повтор".into(),
            );
        }
        super::ring_acceptance::record(ctx, c.family.id, &self.layout, self.pool, initial.1);
        Verdict::Taken(Box::new(Found {
            family: c.family,
            reference: PoolRef::Contract(self.pool),
            token0: self.assets[0],
            token1: self.assets[1],
            coin: c.coin,
            fee_pips: Some(3000),
            tick_spacing: None,
            bin_step: None,
            key: None,
            hooks: None,
        }))
    }
}

/// Quoter может посчитать выход сверх фактического залога fw. Оба чтения в одном
/// eth_call Multicall3 — та же среда EVM, даже при «latest».
pub(in crate::venues::bsc_dex) async fn backed_quote(
    ctx: &Ctx,
    e: &super::registry::Entry,
    token_in: Address,
    amount: U256,
    block: &str,
) -> crate::gateway::errors::GatewayResult<U256> {
    use crate::gateway::errors::ErrorCode;
    use crate::venues::bsc_dex::quote::quoter_calls;
    let output = if token_in == e.pool.token0 {
        e.pool.token1
    } else if token_in == e.pool.token1 {
        e.pool.token0
    } else {
        return Err(ctx.err(ErrorCode::NoRoute, "Ring: вход отсутствует в паре"));
    };
    let quote = quoter_calls::request(ctx, e, token_in, amount)?;
    let layout = ring::checked(ctx, e.family.id, e.pool.reference.address())?;
    let collateral = IRingProof::balanceOfCall {
        owner: layout.wrapped(output),
    };
    let results = aggregate(
        ctx,
        layout.multicall,
        &[quote, (output, Bytes::from(collateral.abi_encode()))],
        &json!(block),
    )
    .await?;
    let [Ok(price), Ok(backing)] = results.as_slice() else {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            "Ring: не прочитана котировка или обеспечение вывода",
        ));
    };
    let out = quoter_calls::decode_input(e, price, token_in, amount)
        .ok_or_else(|| ctx.err(ErrorCode::BadResponse, "Ring: неверный ответ квотера"))?;
    let available = super::super::node::call::decode::<IRingProof::balanceOfCall>(backing)?;
    if out.is_zero() || out > available {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            "Ring: недостаточно обеспечения для этого объёма",
        ));
    }
    Ok(out)
}

pub(super) async fn verify(
    ctx: &Ctx,
    candidates: &[Candidate],
    indices: &[usize],
    out: &mut [Option<Verdict>],
) {
    if indices.is_empty() {
        return;
    }
    #[derive(serde::Deserialize)]
    struct Head {
        hash: B256,
    }
    let initial = (
        ctx.settings.generation(),
        ctx.int("dex.evm_chain_id").unwrap_or(0),
    );
    let tag = rpc::call::<Head>(ctx, "eth_getBlockByNumber", json!(["latest", false]))
        .await
        .map(|h| json!({"blockHash":h.hash,"requireCanonical":true}));
    let mut deployments = std::collections::HashMap::new();
    for &i in indices {
        let c = &candidates[i];
        let layout = match ring::active(ctx, c.family.id) {
            Ok(layout) => layout,
            Err(e) => {
                out[i] = Some(Verdict::Failed(e.message));
                continue;
            }
        };
        if let Ok(tag) = &tag {
            let lookup = (
                c.family.id,
                layout.contracts(),
                layout.infrastructure,
                layout.native,
            );
            if let std::collections::hash_map::Entry::Vacant(entry) = deployments.entry(lookup) {
                entry.insert(infrastructure(ctx, &layout, tag).await);
            }
            if let Err(why) = &deployments[&lookup] {
                out[i] = Some(Verdict::Failed(why.clone()));
                continue;
            }
        }
        let result = match (&tag, Market::from(c, layout)) {
            (Err(e), _) => Verdict::Failed(e.message.clone()),
            (_, None) => Verdict::Rejected(
                "Ring: адрес не совпадает с CREATE2 пары канонических обёрток".into(),
            ),
            (Ok(tag), Some(m)) => m.verdict(ctx, c, tag, initial).await,
        };
        out[i] = Some(result);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enumerated_ring_pairs_resolve_to_canonical_underlying_only() {
        let j: Value = serde_json::from_str(include_str!(
            "../../../../tests/data/first_run_dex_2026-11-26/ring_directory.json"
        ))
        .unwrap();
        assert_eq!(j["count"], 474);
        let id = "ring-exchange-ethereum";
        let layout = ring::Layout::archived(super::ring::archived_profile(1, id).unwrap()).unwrap();
        let family = super::super::families::make(id, "ring_few_v2").unwrap();
        let mut passed = 0;
        let mut wrong = 0;
        for p in j["rows"].as_array().unwrap() {
            let ws = p["wrappers"].as_array().unwrap();
            let token = |i: usize| &j["tokens"][ws[i].as_str().unwrap()];
            let original = |i: usize| token(i)["asset"].as_str()?.parse::<Address>().ok();
            let (Some(a), Some(b)) = (original(0), original(1)) else {
                continue;
            };
            let c = Candidate {
                family,
                pool: p["pool"].as_str().unwrap().into(),
                coin: a,
                quote: b,
            };
            let market = Market::from(&c, layout.clone());
            if p["verified"] == true {
                let m = market.unwrap();
                assert_eq!(m.assets, [a, b]);
                assert_eq!(
                    j["code_hash"][c.pool.as_str()],
                    format!("{:#x}", layout.pair_runtime)
                );
                for (i, fw) in m.fw.into_iter().enumerate() {
                    assert_eq!(ws[i], format!("{fw:#x}"));
                    assert_eq!(
                        j["code_hash"][format!("{fw:#x}")],
                        format!("{:#x}", layout.wrapper_runtime)
                    );
                    assert_eq!(token(i)["verified"], true);
                }
                passed += 1;
            } else if market.is_none() {
                wrong += 1;
            }
        }
        assert_eq!(passed, 97);
        assert!(wrong >= 370);
    }
}

#[test]
fn ring_failed_amount_or_funding_is_not_a_pool_ban() {
    use std::collections::BTreeMap;
    let j: Value = serde_json::from_str(include_str!(
        "../../../../tests/data/first_run_dex_2026-11-26/ring_markets.json"
    ))
    .unwrap();
    let mut counts = BTreeMap::new();
    for row in j["markets"].as_array().unwrap() {
        for case in row["cases"].as_array().unwrap() {
            let status = case["status"].as_str().unwrap();
            *counts.entry(status).or_insert(0) += 1;
            if status == "both_swaps" {
                assert_eq!(
                    case["minimum_controls"],
                    json!(["0x0", "0x0", "0x0", "0x0"])
                );
            } else if status == "reverted" {
                let group = case["failure_group"].as_str().unwrap();
                *counts.entry(group).or_insert(0) += 1;
                let backing = case["available"].as_str().unwrap().parse::<U256>().unwrap();
                let amount = case["requested"].as_str().unwrap().parse::<U256>().unwrap();
                assert_eq!(
                    backing < amount,
                    group != "transfer_refused_despite_nominal_balance"
                );
                assert_eq!(case["stage"] == "fund", group == "funding_backing");
            }
        }
    }
    assert_eq!(counts["both_swaps"], 115);
    assert_eq!(counts["no_funded_positive_size"], 65);
    assert_eq!(counts["reverted"], 14);
    assert_eq!(counts["output_backing"], 8);
    assert_eq!(counts["funding_backing"], 2);
    assert_eq!(counts["transfer_refused_despite_nominal_balance"], 4);
    let mixed = j["markets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["index"] == 132)
        .unwrap();
    assert_eq!(mixed["cases"][0]["status"], "no_funded_positive_size");
    assert_eq!(mixed["cases"][1]["status"], "both_swaps");
    assert_eq!(mixed["cases"][1]["input_raw"], "2");
    assert_eq!(mixed["cases"][1]["out_raw"], "1");
}
