//! DAGrow SSP. Код обоих proxy, реальные валюты, плательщик, ликвидность и ограничения на одной канонической высоте.
use super::{
    families::{self, Family},
    family_variant::Variant,
    nami_batch as batch,
    stable_list::Extra,
    verify::{Candidate, Verdict},
};
use crate::{
    core::types::{DagrowState, PoolRef},
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{
        ctx::Ctx, directory::gt_parse::IndexedPool, node::abi::IERC20, wallet::signer,
    },
};
use alloy::sol;
use alloy_primitives::{Address, B256, U256};
use serde_json::Value;
use std::collections::HashMap;

sol! {interface ISsp{
    function poolGuard() external view returns(address);
    function stablecoin() external view returns(address);
    function dagrToken() external view returns(address);
    function token0() external view returns(address);
    function token1() external view returns(address);
    function stablecoinDecimals() external view returns(uint8);
    function getReserves() external view returns(uint112 reserve0,uint112 reserve1,uint32 timestamp);
    function getAmountsOut(uint256 n,address[] path,address payer) external view returns(uint256[]);
    function getAmountsIn(uint256 n,address[] path,address payer) external view returns(uint256[]);
    function verifySwap(address user,bool isSell) external view returns(uint8);
    function ssp() external view returns(address);
    function exoBps() external view returns(uint256);
    function ecoBps() external view returns(uint256);
    function guardianTradingHalted() external view returns(bool);
    function isSellPaused() external view returns(bool);
    function pool() external view returns(address);
    function stable() external view returns(address);
    function dagr() external view returns(address);
}}
fn bad(ctx: &Ctx) -> crate::gateway::errors::GatewayError {
    ctx.err(
        ErrorCode::NotApplicable,
        "DAGrow: изменились размещение, валюта, комиссия или доступность заданной стороны",
    )
}
async fn proxies(ctx: &Ctx, f: &Family, pool: Address, at: &Value) -> GatewayResult<Address> {
    let id = f.id;
    let guard = ctx.dex_addr("dex.quoter", id)?;
    if pool != ctx.dex_addr("dex.factory", id)?
        || pool == guard
        || pool.is_zero()
        || guard.is_zero()
    {
        return Err(bad(ctx));
    }
    let impls = ctx.dex_list("dex.proxy_implementations", id)?;
    let hashes = ctx.dex_list("dex.proxy_runtime_hashes", id)?;
    let ([p, g], [ph, gh]) = (impls.as_slice(), hashes.as_slice()) else {
        return Err(bad(ctx));
    };
    let check = |addr, key, ip: &String, h: &String| {
        let shell = ctx
            .dex_text(key, id)?
            .parse::<B256>()
            .map_err(|_| bad(ctx))?;
        let logic = ip.parse::<Address>().map_err(|_| bad(ctx))?;
        let hash = h.parse::<B256>().map_err(|_| bad(ctx))?;
        if shell.is_zero() || logic.is_zero() || hash.is_zero() {
            return Err(bad(ctx));
        }
        Ok::<_, crate::gateway::errors::GatewayError>(super::solidly_classic_factory::bind(
            ctx,
            addr,
            shell,
            Some((logic, hash)),
            at,
        ))
    };
    tokio::try_join!(
        check(pool, "dex.factory_runtime_hash", p, ph)?,
        check(guard, "dex.quoter_runtime_hash", g, gh)?
    )?;
    Ok(guard)
}
async fn module(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    stable: Address,
    dagr: Address,
    at: &Value,
) -> GatewayResult<Address> {
    let target = ctx.dex_addr("dex.router", f.id)?;
    let allowed = super::direct_module::configured(ctx, f.id)?;
    let [code, p, s, d] = batch::read(
        ctx,
        [
            ("eth_getCode", serde_json::json!([target, at])),
            batch::view(at, target, ISsp::poolCall {}),
            batch::view(at, target, ISsp::stableCall {}),
            batch::view(at, target, ISsp::dagrCall {}),
        ],
    )
    .await?;
    if super::direct_module::matching_hash(&allowed, &code).is_none()
        || batch::decode(&p, ISsp::poolCall {})? != pool
        || batch::decode(&s, ISsp::stableCall {})? != stable
        || batch::decode(&d, ISsp::dagrCall {})? != dagr
    {
        return Err(bad(ctx));
    }
    Ok(target)
}
pub(crate) async fn allowance(
    ctx: &Ctx,
    asset: Address,
    owner: Address,
    spender: Address,
    at: &Value,
) -> GatewayResult<U256> {
    let q = IERC20::allowanceCall { owner, spender };
    let [v] = batch::read(ctx, [batch::view(at, asset, q.clone())]).await?;
    batch::decode(&v, q)
}
pub(crate) async fn balance(
    ctx: &Ctx,
    asset: Address,
    owner: Address,
    at: &Value,
) -> GatewayResult<U256> {
    let q = IERC20::balanceOfCall { owner };
    let [v] = batch::read(ctx, [batch::view(at, asset, q.clone())]).await?;
    batch::decode(&v, q)
}
pub async fn official(
    ctx: &Ctx,
    s: &DagrowState,
    input: Address,
    by_target: bool,
    amount: U256,
    at: &Value,
) -> GatewayResult<U256> {
    let path = if input == s.stable {
        vec![s.stable, s.dagr]
    } else if input == s.dagr {
        vec![s.dagr, s.stable]
    } else {
        return Err(bad(ctx));
    };
    let values = if by_target {
        let q = ISsp::getAmountsInCall {
            n: amount,
            path,
            payer: s.payer,
        };
        let [v] = batch::read(ctx, [batch::view(at, s.pool, q.clone())]).await?;
        batch::decode(&v, q)?
    } else {
        let q = ISsp::getAmountsOutCall {
            n: amount,
            path,
            payer: s.payer,
        };
        let [v] = batch::read(ctx, [batch::view(at, s.pool, q.clone())]).await?;
        batch::decode(&v, q)?
    };
    let [used, paid] = values.as_slice() else {
        return Err(bad(ctx));
    };
    if used.is_zero() || paid.is_zero() || (if by_target { *paid } else { *used }) != amount {
        return Err(bad(ctx));
    }
    Ok(if by_target { *used } else { *paid })
}
pub(crate) async fn state(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    expected: Option<(Address, Address)>,
    at: &Value,
) -> GatewayResult<DagrowState> {
    if at["requireCanonical"] != true
        || at["blockHash"]
            .as_str()
            .and_then(|h| h.parse::<B256>().ok())
            .is_none()
    {
        return Err(bad(ctx));
    }
    let generation = ctx.settings.generation();
    let guard = proxies(ctx, f, pool, at).await?;
    let [g, p, a, b, x, y, dec, res, eco, exo, halt, pause] = batch::read(
        ctx,
        [
            batch::view(at, pool, ISsp::poolGuardCall {}),
            batch::view(at, guard, ISsp::sspCall {}),
            batch::view(at, pool, ISsp::stablecoinCall {}),
            batch::view(at, pool, ISsp::dagrTokenCall {}),
            batch::view(at, pool, ISsp::token0Call {}),
            batch::view(at, pool, ISsp::token1Call {}),
            batch::view(at, pool, ISsp::stablecoinDecimalsCall {}),
            batch::view(at, pool, ISsp::getReservesCall {}),
            batch::view(at, guard, ISsp::ecoBpsCall {}),
            batch::view(at, guard, ISsp::exoBpsCall {}),
            batch::view(at, guard, ISsp::guardianTradingHaltedCall {}),
            batch::view(at, guard, ISsp::isSellPausedCall {}),
        ],
    )
    .await?;
    let stable = batch::decode(&a, ISsp::stablecoinCall {})?;
    let dagr = batch::decode(&b, ISsp::dagrTokenCall {})?;
    let tokens = (
        batch::decode(&x, ISsp::token0Call {})?,
        batch::decode(&y, ISsp::token1Call {})?,
    );
    let same = |pair| pair == (stable, dagr) || pair == (dagr, stable);
    if stable.is_zero()
        || dagr.is_zero()
        || stable == dagr
        || tokens != (stable, dagr)
        || expected.is_some_and(|pair| !same(pair))
        || batch::decode(&g, ISsp::poolGuardCall {})? != guard
        || batch::decode(&p, ISsp::sspCall {})? != pool
    {
        return Err(bad(ctx));
    }
    let cash_dec = batch::decode(&dec, ISsp::stablecoinDecimalsCall {})?;
    let [sc, dc, sb, db] = batch::read(
        ctx,
        [
            batch::view(at, stable, IERC20::decimalsCall {}),
            batch::view(at, dagr, IERC20::decimalsCall {}),
            batch::view(at, stable, IERC20::balanceOfCall { owner: pool }),
            batch::view(at, dagr, IERC20::balanceOfCall { owner: pool }),
        ],
    )
    .await?;
    if cash_dec > 18
        || batch::decode(&sc, IERC20::decimalsCall {})? != cash_dec
        || batch::decode(&dc, IERC20::decimalsCall {})? != 18
    {
        return Err(bad(ctx));
    }
    let rs = batch::decode(&res, ISsp::getReservesCall {})?;
    let reserves = [U256::from(rs.reserve0), U256::from(rs.reserve1)];
    let balances = [
        batch::decode(&sb, IERC20::balanceOfCall { owner: pool })?,
        batch::decode(&db, IERC20::balanceOfCall { owner: pool })?,
    ];
    let scale = U256::from(10u64.pow(u32::from(18 - cash_dec)));
    let payer = if f.executor_kind.is_some() {
        module(ctx, f, pool, stable, dagr, at).await?
    } else {
        signer::address(ctx).unwrap_or_default()
    };
    let rates = [
        batch::decode(&eco, ISsp::ecoBpsCall {})?,
        batch::decode(&exo, ISsp::exoBpsCall {})?,
    ];
    if rates.iter().any(|r| *r >= U256::from(10_000)) {
        return Err(bad(ctx));
    }
    let stopped = batch::decode(&halt, ISsp::guardianTradingHaltedCall {})?;
    let sell_paused = batch::decode(&pause, ISsp::isSellPausedCall {})?;
    let mut fees = [None, None];
    if !stopped && !payer.is_zero() {
        for (side, slot) in fees.iter_mut().enumerate() {
            if side == 1 && sell_paused {
                continue;
            }
            let q = ISsp::verifySwapCall {
                user: payer,
                isSell: side == 1,
            };
            if let Ok([v]) = batch::read(ctx, [batch::view(at, guard, q.clone())]).await {
                let tier = batch::decode(&v, q)?;
                let rate = rates.get(usize::from(tier)).ok_or_else(|| bad(ctx))?;
                *slot = Some(rate.to());
            }
        }
    }
    if generation != ctx.settings.generation() {
        return Err(bad(ctx));
    }
    Ok(DagrowState {
        pool,
        stable,
        dagr,
        payer,
        reserves,
        balances,
        stable_scale: scale,
        fees,
    })
}
pub(super) async fn verify(ctx: &Ctx, c: &Candidate) -> Verdict {
    let Some(pool) = c.pool.parse().ok() else {
        return Verdict::Rejected("DAGrow: нужен адрес SSP".into());
    };
    let result = async {
        let at = super::nami::block(ctx, "latest").await?;
        state(ctx, c.family, pool, Some((c.coin, c.quote)), &at).await
    }
    .await;
    match result {
        Ok(s) => {
            let reference = if c.family.executor_kind.is_some() {
                PoolRef::Multi {
                    contract: pool,
                    token0: s.stable.min(s.dagr),
                    token1: s.stable.max(s.dagr),
                }
            } else {
                PoolRef::Contract(pool)
            };
            Verdict::Taken(Box::new(super::verify_woo::found(c, reference)))
        }
        Err(e) if e.code == ErrorCode::NotApplicable => Verdict::Rejected(e.message),
        Err(e) => Verdict::Failed(e.message),
    }
}
pub(super) async fn scan(ctx: &Ctx, coins: &HashMap<Address, String>, quotes: &[Address]) -> Extra {
    let mut out = Extra::default();
    for f in families::all(ctx)
        .into_iter()
        .filter(|f| f.variant == Variant::Dagrow)
    {
        if families::classify(ctx, f.id).is_err() {
            continue;
        }
        let result = async {
            let pool = ctx.dex_addr("dex.factory", f.id)?;
            let at = super::nami::block(ctx, "latest").await?;
            state(ctx, f, pool, None, &at).await
        }
        .await;
        match result {
            Ok(s) => {
                out.calls += 1;
                for (a, b) in [(s.stable, s.dagr), (s.dagr, s.stable)] {
                    if let Some(symbol) = coins.get(&a)
                        && quotes.contains(&b)
                    {
                        out.found.entry(a).or_default().push(IndexedPool {
                            dex_id: f.id.into(),
                            pool: s.pool.to_string(),
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
                out.notes.push(format!("{}: {}", f.id, e.message));
            }
        }
    }
    out
}
