//! Публичная пара DefiPlaza. Регистрация, обе реальные кассы и условия обмена читаются на одном H.
use super::{
    families::{self, Family},
    nami_batch as batch,
    stable_list::Extra,
    verify::{Candidate, Verdict},
};
use crate::{
    core::types::{
        PoolRef,
        defiplaza::{DefiPlazaCurve, DefiPlazaState},
    },
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, directory::gt_parse::IndexedPool, node::rpc},
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, U256};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

const STABLE_ASSETS: u8 = 4;
// DeFiPlaza.sol:20, enum State {Unlisted, PreListing, Delisting, Listed}.
// https://sourcify.dev/server/v2/contract/1/0xE68c1d72340aEeFe5Be76eDa63AE2f4bc7514110?fields=sources
const LISTED: u8 = 3;

#[allow(clippy::too_many_arguments)]
mod abi {
    use alloy::sol;
    sol! { interface IPlaza {
        function DFPconfig() external view returns(bool unlocked,uint64 oneMinusTradingFee,uint64 delistingBonus);
        function listedTokens(address token) external view returns(uint8 status,uint112 target);
        function tokens(uint256 i) external view returns(address token,uint64 denorm);
        function reserves(uint256 i) external view returns(uint64);
        function getIndex(address token) external view returns(uint256);
        function SPconfig() external view returns(uint16 locked,uint8 feeLevel,uint8 flashLoanFeeLevel,uint8 stakerFeeFraction,uint8 maxLockingBonus,uint16 maxLockingTime,uint64 delta,uint64 rewards,uint64 supply);
        function quote(address tokenIn,address tokenOut,uint256 amount) external view returns(uint256);
        function exchange() external view returns(address);
        function wrapped() external view returns(address);
        function stable() external view returns(bool);
        event Swapped(address sender,address inputToken,address outputToken,uint256 inputAmount,uint256 outputAmount);
        event Swap(address sender,address inputToken,address outputToken,uint256 inputAmount,uint256 outputAmount,address destination);
    } }
}
pub(in crate::venues::bsc_dex) use abi::IPlaza;
fn bad(ctx: &Ctx) -> crate::gateway::errors::GatewayError {
    ctx.err(
        ErrorCode::NotApplicable,
        "DefiPlaza: касса, валюта или условия обмена изменились",
    )
}
async fn read<C: SolCall + Clone>(
    ctx: &Ctx,
    target: Address,
    at: &Value,
    c: C,
) -> GatewayResult<C::Return> {
    let [v] = batch::read(ctx, [batch::view(at, target, c.clone())]).await?;
    batch::decode(&v, c)
}
async fn cash(ctx: &Ctx, a: Address, pool: Address, at: &Value) -> GatewayResult<U256> {
    if a.is_zero() {
        rpc::call(ctx, "eth_getBalance", json!([pool, at])).await
    } else {
        super::dagrow::balance(ctx, a, pool, at).await
    }
}
async fn token(
    ctx: &Ctx,
    pool: Address,
    a: Address,
    wrapped: Address,
    stable: bool,
    at: &Value,
) -> GatewayResult<(Address, U256, U256, U256)> {
    if !stable {
        let src = if a == wrapped { Address::ZERO } else { a };
        let active = async {
            if !src.is_zero()
                && read(ctx, pool, at, IPlaza::listedTokensCall { token: src })
                    .await?
                    .status
                    != LISTED
            {
                return Err(bad(ctx));
            }
            Ok(())
        };
        let (balance, ()) = tokio::try_join!(cash(ctx, src, pool, at), active)?;
        return Ok((src, balance, balance, U256::ONE));
    }
    let idx = read(ctx, pool, at, IPlaza::getIndexCall { token: a }).await?;
    if idx >= U256::from(STABLE_ASSETS) {
        return Err(bad(ctx));
    }
    let (info, reserve, balance) = tokio::try_join!(
        read(ctx, pool, at, IPlaza::tokensCall { i: idx }),
        read(ctx, pool, at, IPlaza::reservesCall { i: idx }),
        cash(ctx, a, pool, at)
    )?;
    if info.token != a || info.denorm == 0 {
        return Err(bad(ctx));
    }
    Ok((a, balance, U256::from(reserve), U256::from(info.denorm)))
}
pub(crate) async fn state(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    pair: (Address, Address),
    at: &Value,
) -> GatewayResult<DefiPlazaState> {
    let generation = ctx.settings.generation();
    let stable = f.variant.defiplaza_stable();
    let hash = ctx
        .dex_text("dex.factory_runtime_hash", f.id)?
        .parse::<B256>()
        .map_err(|_| bad(ctx))?;
    let module = ctx.dex_addr("dex.router", f.id)?;
    let wrapped = ctx.addr("dex.native_wrapped")?;
    if pool != ctx.dex_addr("dex.factory", f.id)?
        || hash.is_zero()
        || module.is_zero()
        || pool == module
        || pair.0 == pair.1
        || pair.0.is_zero()
        || pair.1.is_zero()
    {
        return Err(bad(ctx));
    }
    let accepted = super::direct_module::configured(ctx, f.id)?;
    let check = async {
        let [code, p, w, s] = batch::read(
            ctx,
            [
                ("eth_getCode", json!([module, at])),
                batch::view(at, module, IPlaza::exchangeCall {}),
                batch::view(at, module, IPlaza::wrappedCall {}),
                batch::view(at, module, IPlaza::stableCall {}),
            ],
        )
        .await?;
        let passed =
            super::direct_module::matching_hash(&accepted, &code).ok_or_else(|| bad(ctx))?;
        if batch::decode(&p, IPlaza::exchangeCall {})? != pool
            || batch::decode(&w, IPlaza::wrappedCall {})? != wrapped
            || batch::decode(&s, IPlaza::stableCall {})? != stable
        {
            return Err(bad(ctx));
        }
        Ok(passed)
    };
    let settings = async {
        if stable {
            let c = read(ctx, pool, at, IPlaza::SPconfigCall {}).await?;
            if c.locked != 0 {
                return Err(bad(ctx));
            }
            Ok(DefiPlazaCurve::Stable {
                fee_bps: c.feeLevel,
                delta: c.delta,
            })
        } else {
            let c = read(ctx, pool, at, IPlaza::DFPconfigCall {}).await?;
            if !c.unlocked || c.oneMinusTradingFee == 0 {
                return Err(bad(ctx));
            }
            Ok(DefiPlazaCurve::Multi {
                one_minus_fee: c.oneMinusTradingFee,
            })
        }
    };
    let tokens = [pair.0.min(pair.1), pair.0.max(pair.1)];
    let ((), module_hash, curve, left, right) = tokio::try_join!(
        super::solidly_classic_factory::bind(ctx, pool, hash, None, at),
        check,
        settings,
        token(ctx, pool, tokens[0], wrapped, stable, at),
        token(ctx, pool, tokens[1], wrapped, stable, at)
    )?;
    if ctx.settings.generation() != generation {
        return Err(bad(ctx));
    }
    Ok(DefiPlazaState {
        pool,
        module,
        module_hash,
        tokens,
        physical: [left.0, right.0],
        balances: [left.1, right.1],
        reserves: [left.2, right.2],
        factors: [left.3, right.3],
        curve,
    })
}

pub(super) async fn verify(ctx: &Ctx, c: &Candidate) -> Verdict {
    let Some(pool) = c.pool.parse().ok() else {
        return Verdict::Rejected("DefiPlaza: нужен адрес кассы".into());
    };
    let result = async {
        let at = super::nami::block(ctx, "latest").await?;
        state(ctx, c.family, pool, (c.coin, c.quote), &at).await
    }
    .await;
    match result {
        Ok(s) => Verdict::Taken(Box::new(super::verify_woo::found(
            c,
            PoolRef::Multi {
                contract: pool,
                token0: s.tokens[0],
                token1: s.tokens[1],
            },
        ))),
        Err(e) if e.code == ErrorCode::NotApplicable => Verdict::Rejected(e.message),
        Err(e) => Verdict::Failed(e.message),
    }
}

/// Группы независимых listedTokens на одном заголовке, включая последнюю неполную пачку.
fn registry_requests<'a>(
    pool: Address,
    at: &'a Value,
    assets: &'a [Address],
    size: usize,
) -> impl Iterator<Item = (&'a [Address], Vec<batch::Request>)> {
    assets.chunks(size).map(move |group| {
        let calls = group.iter()
            .map(|&token| batch::view(at, pool, IPlaza::listedTokensCall { token }))
            .collect();
        (group, calls)
    })
}

pub(super) async fn scan(ctx: &Ctx, coins: &HashMap<Address, String>, quotes: &[Address]) -> Extra {
    let mut out = Extra::default();
    let candidates: HashSet<_> = coins.keys().chain(quotes).copied().collect();
    for f in families::all(ctx)
        .into_iter()
        .filter(|f| f.variant.defiplaza())
    {
        if families::classify(ctx, f.id).is_err() {
            continue;
        }
        let result = async {
            let pool = ctx.dex_addr("dex.factory", f.id)?;
            let at = super::nami::block(ctx, "latest").await?;
            let mut listed = HashSet::new();
            if f.variant.defiplaza_stable() {
                for i in 0..STABLE_ASSETS {
                    listed.insert(
                        read(ctx, pool, &at, IPlaza::tokensCall { i: U256::from(i) })
                            .await?
                            .token,
                    );
                }
            } else {
                let weth = ctx.addr("dex.native_wrapped")?;
                listed.insert(weth);
                let assets: Vec<_> = candidates
                    .iter()
                    .copied()
                    .filter(|a| !a.is_zero() && *a != weth)
                    .collect();
                let size = super::load_batch::batch(ctx)?;
                for (group, calls) in registry_requests(pool, &at, &assets, size) {
                    let results = rpc::batch(ctx, &calls).await?;
                    for (&a, value) in group.iter().zip(results) {
                        let data: alloy_primitives::Bytes =
                            serde_json::from_str(value?.get()).map_err(|_| bad(ctx))?;
                        if batch::decode(&data, IPlaza::listedTokensCall { token: a })?.status
                            == LISTED
                        {
                            listed.insert(a);
                        }
                    }
                }
            }
            Ok::<_, crate::gateway::errors::GatewayError>((pool, listed))
        }
        .await;
        match result {
            Ok((pool, listed)) => {
                out.calls += 1;
                for (&a, symbol) in coins.iter().filter(|(a, _)| listed.contains(*a)) {
                    for &b in quotes.iter().filter(|b| **b != a && listed.contains(*b)) {
                        out.found.entry(a).or_default().push(IndexedPool {
                            dex_id: f.id.into(),
                            pool: pool.to_string(),
                            base: a,
                            quote: b,
                            base_symbol: symbol.clone(),
                            quote_symbol: coins
                                .get(&b)
                                .cloned()
                                .unwrap_or_else(|| format!("{b:#x}")),
                            reserve_usd: None,
                        });
                    }
                }
            }
            Err(e) => {
                out.no_answer += 1;
                out.notes.push(e.message);
            }
        }
    }
    out
}

pub(crate) fn request(
    ctx: &Ctx,
    s: &DefiPlazaState,
    input: Address,
    amount: U256,
) -> GatewayResult<(Address, alloy_primitives::Bytes)> {
    let i = s
        .tokens
        .iter()
        .position(|a| *a == input)
        .ok_or_else(|| bad(ctx))?;
    Ok((
        s.module,
        IPlaza::quoteCall {
            tokenIn: input,
            tokenOut: s.tokens[1 - i],
            amount,
        }
        .abi_encode()
        .into(),
    ))
}
pub(crate) async fn out_at(
    ctx: &Ctx,
    e: &super::registry::Entry,
    input: Address,
    amount: U256,
    at: &Value,
) -> GatewayResult<U256> {
    let s = Box::pin(state(
        ctx,
        e.family,
        e.pool.reference.address(),
        (e.pool.token0, e.pool.token1),
        at,
    ))
    .await?;
    let (module, data) = request(ctx, &s, input, amount)?;
    let val: alloy_primitives::Bytes =
        rpc::call(ctx, "eth_call", json!([{"to":module,"data":data},at])).await?;
    let quote = IPlaza::quoteCall::abi_decode_returns(&val).map_err(|_| bad(ctx))?;
    if !s
        .swap(input, true, amount)
        .is_some_and(|f| f.amount_out == quote)
    {
        return Err(bad(ctx));
    }
    Ok(quote)
}


#[cfg(test)]
mod registry_tests {
    use super::*;
    use alloy_primitives::Bytes;

    #[test]
    fn each_rpc_page_preserves_all_recorded_registrations_and_their_block() {
        let bytes = include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/defiplaza_multi_ethereum.json.gz");
        let book: Value = serde_json::from_reader(flate2::read::GzDecoder::new(bytes.as_slice())).unwrap();
        let pool = book["pool"].as_str().unwrap().parse().unwrap();
        let at = &book["selector"];
        let assets: Vec<Address> = book["inventory"]["registration"]["states"].as_object().unwrap()
            .keys().map(|token| token.parse().unwrap()).collect();
        assert_eq!(assets.len(), 15);
        for (size, expected) in [(4, vec![4,4,4,3]), (15, vec![15]), (48, vec![15]), (1, vec![1;15])] {
            let mut lengths = Vec::new();
            let mut seen = Vec::new();
            for (tokens, calls) in registry_requests(pool, at, &assets, size) {
                lengths.push(calls.len());
                assert_eq!(tokens.len(), calls.len());
                for (&token, (method, params)) in tokens.iter().zip(calls) {
                    assert_eq!(method, "eth_call");
                    assert_eq!(params[0]["to"], book["pool"]);
                    assert_eq!(&params[1], at);
                    let data: Bytes = serde_json::from_value(params[0]["data"].clone()).unwrap();
                    assert_eq!(IPlaza::listedTokensCall::abi_decode(&data).unwrap().token, token);
                    seen.push(token);
                }
            }
            assert_eq!(lengths, expected);
            assert_eq!(seen, assets);
        }
    }
}
