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
use alloy_primitives::{Address, B256, Bytes, U256, b256, keccak256};
use serde_json::{Value, json};

sol! {
    interface IRingProof {
        function getWrappedToken(address asset) external view returns (address);
        function getPair(address a, address b) external view returns (address);
        function factory() external view returns (address);
        function token() external view returns (address);
        function token0() external view returns (address);
        function token1() external view returns (address);
        function decimals() external view returns (uint8);
        function balanceOf(address owner) external view returns (uint256);
    }
}

const PAIR_RUNTIME: B256 =
    b256!("330be2c0f89c41b8879b8015445a667347764915107aced846fd148cc7c03bc0");
const FW_RUNTIME: B256 = b256!("be63f68c8de6e3f7cf411d9b117b931128468c36e0753fc5878fd5359f16acf5");

const MC: Address = alloy_primitives::address!("ca11bde05977b3631167028862be2a173976ca11");

fn view<C: SolCall>(target: Address, method: C) -> (Address, Bytes) {
    (target, Bytes::from(method.abi_encode()))
}

async fn aggregate(
    ctx: &Ctx,
    calls: &[(Address, Bytes)],
    tag: &Value,
) -> crate::gateway::errors::GatewayResult<Vec<Result<Bytes, String>>> {
    use crate::venues::bsc_dex::node::multicall;
    let data = multicall::encode(calls);
    let bytes: Bytes = rpc::call(ctx, "eth_call", json!([{"to":MC,"input":data},tag])).await?;
    multicall::decode(&bytes, calls.len())
}

async fn code(ctx: &Ctx, a: Address, tag: &Value) -> crate::gateway::errors::GatewayResult<B256> {
    let raw: Bytes = rpc::call(ctx, "eth_getCode", json!([a, tag])).await?;
    Ok(keccak256(raw))
}

struct Market {
    pool: Address,
    assets: [Address; 2],
    fw: [Address; 2],
}

impl Market {
    fn from(c: &Candidate) -> Option<Self> {
        let pool = c.pool.parse().ok()?;
        let [a, b] = ring::path(c.coin, c.quote)?;
        if !ring::check_pool(pool, a, b) {
            return None;
        }
        let (assets, fw) = if a < b {
            ([c.coin, c.quote], [a, b])
        } else {
            ([c.quote, c.coin], [b, a])
        };
        Some(Self { pool, assets, fw })
    }
    fn requests(&self) -> Vec<(Address, Bytes)> {
        let mut calls = vec![
            view(self.pool, IRingProof::factoryCall {}),
            view(self.pool, IRingProof::token0Call {}),
            view(self.pool, IRingProof::token1Call {}),
            view(
                ring::FACTORY,
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
                view(ring::FEW, IRingProof::getWrappedTokenCall { asset }),
                view(fw, IRingProof::decimalsCall {}),
                view(asset, IRingProof::decimalsCall {}),
            ]);
        }
        calls
    }

    fn check(&self, rows: &[Bytes]) -> bool {
        let addr = |data: &Bytes, a: Address| data.as_ref() == a.into_word().as_slice();
        let [factory, t0, t1, pair, wrappers @ ..] = rows else {
            return false;
        };
        if !addr(factory, ring::FACTORY)
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
                addr(&r[0], ring::FEW)
                    && addr(&r[1], asset)
                    && addr(&r[2], fw)
                    && r[3].len() == 32
                    && r[3] == r[4]
                    && U256::from_be_slice(&r[3]) <= U256::from(77)
            })
    }

    async fn verdict(&self, ctx: &Ctx, c: &Candidate, tag: &Value) -> Verdict {
        let calls = self.requests();
        let response = tokio::try_join!(
            code(ctx, self.pool, tag),
            code(ctx, self.fw[0], tag),
            code(ctx, self.fw[1], tag),
            aggregate(ctx, &calls, tag)
        );
        let (pool, fw0, fw1, words) = match response {
            Ok(v) => v,
            Err(e) => return Verdict::Failed(e.message),
        };
        let Ok(words) = words.into_iter().collect::<Result<Vec<_>, _>>() else {
            return Verdict::Rejected("Ring: ошибка чтения связанных контрактов".into());
        };
        if pool != PAIR_RUNTIME || fw0 != FW_RUNTIME || fw1 != FW_RUNTIME || !self.check(&words) {
            return Verdict::Rejected(
                "Ring: код, реестр, исходный токен или decimals расходятся".into(),
            );
        }
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
    let collateral = IRingProof::balanceOfCall {
        owner: ring::wrapped(output),
    };
    let results = aggregate(
        ctx,
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
    let tag = rpc::call::<Head>(ctx, "eth_getBlockByNumber", json!(["latest", false]))
        .await
        .map(|h| json!({"blockHash":h.hash,"requireCanonical":true}));
    for &i in indices {
        let c = &candidates[i];
        let result = match (&tag, Market::from(c)) {
            (Err(e), _) => Verdict::Failed(e.message.clone()),
            (_, None) => Verdict::Rejected(
                "Ring: адрес не совпадает с CREATE2 пары канонических обёрток".into(),
            ),
            (Ok(tag), Some(m)) => m.verdict(ctx, c, tag).await,
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
        let family = super::super::families::make("ring", "ring_few_v2").unwrap();
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
            let market = Market::from(&c);
            if p["verified"] == true {
                let m = market.unwrap();
                assert_eq!(m.assets, [a, b]);
                assert_eq!(
                    j["code_hash"][c.pool.as_str()],
                    format!("{PAIR_RUNTIME:#x}")
                );
                for (i, fw) in m.fw.into_iter().enumerate() {
                    assert_eq!(ws[i], format!("{fw:#x}"));
                    assert_eq!(
                        j["code_hash"][format!("{fw:#x}")],
                        format!("{FW_RUNTIME:#x}")
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
