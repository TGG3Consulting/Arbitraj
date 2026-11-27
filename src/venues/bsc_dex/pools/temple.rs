//! TempleStableAMMRouter. Only registered pairs have a market. Its reserve branch and its defence branch can pay different tokens.
use super::super::{ctx::Ctx, node::rpc};
use super::{families::Family, nami_batch as batch, registry::Entry};
use crate::{
    core::types::{PoolState, TempleState},
    gateway::errors::{ErrorCode, GatewayResult},
};
use alloy::{sol, sol_types::SolCall};
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use serde_json::{Value, json};
sol! {
    interface ITemple {
        function templeToken() external view returns(address);
        function templeTreasury() external view returns(address);
        function defendStable() external view returns(address);
        function tokenPair(address stable) external view returns(address);
        function token0() external view returns(address);
        function token1() external view returns(address);
        function router() external view returns(address);
        function pair() external view returns(address);
        function balanceOf(address who) external view returns(uint256);
        function getReserves() external view returns(uint112 temple,uint112 stable,uint32 last);
        function intrinsicValueRatio() external view returns(uint256 frax,uint256 temple);
        function swapExactStableForTempleQuote(address pair,uint256 amountIn) external view returns(uint256);
        function swapExactTempleForStableQuote(address pair,uint256 amountIn) external view returns(bool priceBelowIV,uint256 amountOut);
    }
}
fn fail(ctx: &Ctx) -> crate::gateway::errors::GatewayError {
    ctx.err(
        ErrorCode::NotApplicable,
        "TempleDAO: пара, актив выплаты или код контрактов не подтверждены на блоке",
    )
}
fn runtime(ctx: &Ctx, f: &Family, k: &str) -> GatewayResult<B256> {
    let h = ctx
        .dex_text(k, f.id)?
        .parse::<B256>()
        .map_err(|_| fail(ctx))?;
    if h.is_zero() || h == keccak256([]) {
        return Err(fail(ctx));
    }
    Ok(h)
}
pub(crate) async fn module(
    ctx: &Ctx,
    f: &Family,
    pair: Address,
    router: Address,
    at: &Value,
) -> GatewayResult<Address> {
    let m = ctx.dex_addr("dex.router", f.id)?;
    let hashes = super::direct_module::configured(ctx, f.id)?;
    let [code, p, r] = batch::read(
        ctx,
        [
            ("eth_getCode", json!([m, at])),
            batch::view(at, m, ITemple::pairCall {}),
            batch::view(at, m, ITemple::routerCall {}),
        ],
    )
    .await?;
    if super::direct_module::matching_hash(&hashes, &code).is_none()
        || batch::decode(&p, ITemple::pairCall {})? != pair
        || batch::decode(&r, ITemple::routerCall {})? != router
    {
        return Err(fail(ctx));
    }
    Ok(m)
}
pub(crate) async fn market(
    ctx: &Ctx,
    f: &Family,
    pair: Address,
    edge: (Address, Address),
    at: &Value,
) -> GatewayResult<TempleState> {
    if pair.is_zero()
        || at["requireCanonical"] != true
        || at["blockHash"]
            .as_str()
            .and_then(|s| s.parse::<B256>().ok())
            .is_none()
    {
        return Err(fail(ctx));
    }
    let generation = ctx.settings.generation();
    let router = ctx.dex_addr("dex.quoter", f.id)?;
    let pair_code = runtime(ctx, f, "dex.pair_runtime_hash")?;
    let router_code = runtime(ctx, f, "dex.router_runtime_hash")?;
    let [rc, pc, allowed, t0, t1, base, def, tre, res] = batch::read(
        ctx,
        [
            ("eth_getCode", json!([router, at])),
            ("eth_getCode", json!([pair, at])),
            batch::view(at, pair, ITemple::routerCall {}),
            batch::view(at, pair, ITemple::token0Call {}),
            batch::view(at, pair, ITemple::token1Call {}),
            batch::view(at, router, ITemple::templeTokenCall {}),
            batch::view(at, router, ITemple::defendStableCall {}),
            batch::view(at, router, ITemple::templeTreasuryCall {}),
            batch::view(at, pair, ITemple::getReservesCall {}),
        ],
    )
    .await?;
    let temple = batch::decode(&base, ITemple::templeTokenCall {})?;
    let stable = batch::decode(&t1, ITemple::token1Call {})?;
    let defend = batch::decode(&def, ITemple::defendStableCall {})?;
    let treasury = batch::decode(&tre, ITemple::templeTreasuryCall {})?;
    let cash = if edge.0 == temple {
        edge.1
    } else if edge.1 == temple {
        edge.0
    } else {
        return Err(fail(ctx));
    };
    if cash.is_zero()
        || cash == temple
        || stable.is_zero()
        || stable == temple
        || temple.is_zero()
        || (cash != stable && cash != defend)
        || keccak256(rc) != router_code
        || keccak256(pc) != pair_code
        || batch::decode(&t0, ITemple::token0Call {})? != temple
    {
        return Err(fail(ctx));
    }
    let authorized = batch::decode(&allowed, ITemple::routerCall {})? == router;
    let res = batch::decode(&res, ITemple::getReservesCall {})?;
    let bal = |asset, who| batch::view(at, asset, ITemple::balanceOfCall { who });
    let requested = [
        batch::view(at, router, ITemple::tokenPairCall { stable }),
        bal(temple, pair),
        bal(stable, pair),
        batch::view(at, treasury, ITemple::intrinsicValueRatioCall {}),
        bal(defend, router),
    ];
    let rows = rpc::batch(ctx, &requested).await?;
    let mut rows = rows.into_iter().map(|r| {
        r.ok()
            .and_then(|v| serde_json::from_str::<Bytes>(v.get()).ok())
    });
    let mut take = || rows.next().flatten();
    let [reg, b0, b1, ratio, balance] = std::array::from_fn(|_| take());
    let reg = reg.ok_or_else(|| fail(ctx))?;
    if batch::decode(&reg, ITemple::tokenPairCall { stable })? != pair {
        return Err(fail(ctx));
    }
    let read = |r: Option<Bytes>| {
        r.and_then(|v| ITemple::balanceOfCall::abi_decode_returns_validate(&v).ok())
    };
    let balances = [
        read(b0).ok_or_else(|| fail(ctx))?,
        read(b1).ok_or_else(|| fail(ctx))?,
    ];
    let ratio = ratio
        .and_then(|v| ITemple::intrinsicValueRatioCall::abi_decode_returns_validate(&v).ok())
        .filter(|r| !r.temple.is_zero())
        .map(|r| [r.frax, r.temple]);
    if f.executor_kind.is_some() {
        module(ctx, f, pair, router, at).await?;
    }
    if generation != ctx.settings.generation() {
        return Err(ctx.err(
            ErrorCode::StaleData,
            "TempleDAO: профиль изменился во время чтения рынка",
        ));
    }
    Ok(TempleState {
        pair,
        router,
        temple,
        stable,
        defend,
        reserves: [U256::from(res.temple), U256::from(res.stable)],
        balances,
        ratio,
        defense_balance: read(balance).unwrap_or_default(),
        amm_authorized: authorized,
    })
}
fn opposite(e: &Entry, input: Address) -> Option<Address> {
    if e.pool.token0 == input {
        Some(e.pool.token1)
    } else if e.pool.token1 == input {
        Some(e.pool.token0)
    } else {
        None
    }
}
pub(crate) fn request(
    ctx: &Ctx,
    e: &Entry,
    input: Address,
    amount: U256,
) -> GatewayResult<(Address, Bytes)> {
    let PoolState::Temple(s) = &e.pool.state else {
        return Err(fail(ctx));
    };
    let out = opposite(e, input).ok_or_else(|| fail(ctx))?;
    let data = if input == s.stable && out == s.temple && s.amm_authorized {
        ITemple::swapExactStableForTempleQuoteCall {
            pair: s.pair,
            amountIn: amount,
        }
        .abi_encode()
    } else if input == s.temple && (out == s.stable || out == s.defend) {
        ITemple::swapExactTempleForStableQuoteCall {
            pair: s.pair,
            amountIn: amount,
        }
        .abi_encode()
    } else {
        return Err(fail(ctx));
    };
    Ok((s.router, data.into()))
}
pub(crate) fn payout(e: &Entry, input: Address, amount: U256, raw: &[u8]) -> Option<U256> {
    let PoolState::Temple(s) = &e.pool.state else {
        return None;
    };
    let out = opposite(e, input)?;
    checked_payout(s, input, out, amount, raw)
}
fn checked_payout(
    s: &TempleState,
    input: Address,
    out: Address,
    amount: U256,
    raw: &[u8],
) -> Option<U256> {
    let observed = if input == s.stable && out == s.temple && s.amm_authorized {
        ITemple::swapExactStableForTempleQuoteCall::abi_decode_returns_validate(raw)
            .ok()
            .filter(|n| !n.is_zero())
    } else if input == s.temple {
        let r =
            ITemple::swapExactTempleForStableQuoteCall::abi_decode_returns_validate(raw).ok()?;
        let legal = if r.priceBelowIV {
            out == s.defend && s.defense_balance >= r.amountOut
        } else {
            out == s.stable && s.amm_authorized
        };
        (legal && !r.amountOut.is_zero()).then_some(r.amountOut)
    } else {
        None
    };
    let expected = super::super::math::temple::quote(s, input, out, true, amount)?;
    observed.filter(|n| *n == expected.amount_out)
}

/// The private AMM emits Swap. The IV branch instead burns TEMPLE and pays the router's defence asset.
pub(in crate::venues::bsc_dex) fn defended(
    e: &Entry,
    entries: &[&Entry],
    logs: &[super::super::check::simulate::SimLog],
) -> bool {
    use super::mint_club::IBond::Transfer;
    use alloy::sol_types::SolEvent;
    let PoolState::Temple(s) = &e.pool.state else {
        return false;
    };
    if ![e.pool.token0, e.pool.token1].contains(&s.defend) || s.defend.is_zero() {
        return false;
    }
    // Events on the defence branch have no pair ID. One router's payment cannot prove two distinct pairs.
    if entries
        .iter()
        .filter(|e| matches!(&e.pool.state,PoolState::Temple(m) if m.router==s.router))
        .count()
        != 1
    {
        return false;
    }
    let is_transfer = |log: &super::super::check::simulate::SimLog| {
        log.topics.first() == Some(&Transfer::SIGNATURE_HASH)
            && log.topics.len() == 3
            && log.data.len() == 32
            && !U256::from_be_slice(&log.data).is_zero()
    };
    for (n, burn) in logs.iter().enumerate() {
        if burn.address != s.temple
            || !is_transfer(burn)
            || !burn.topics[2].is_zero()
            || burn.topics[1].is_zero()
        {
            continue;
        }
        // swapFrom funds the bound module before the router burns that same module's full input.
        let funded = logs[..n].iter().any(|l| {
            l.address == s.temple
                && is_transfer(l)
                && l.topics[2] == burn.topics[1]
                && l.topics[1] != l.topics[2]
                && l.data == burn.data
        });
        let paid = logs[n + 1..].iter().any(|l| {
            l.address == s.defend
                && is_transfer(l)
                && l.topics[1] == s.router.into_word()
                && !l.topics[2].is_zero()
        });
        if funded && paid {
            return true;
        }
    }
    false
}

pub(in crate::venues::bsc_dex) async fn probe_state(
    ctx: &Ctx,
    e: &Entry,
    number: u64,
) -> GatewayResult<(super::super::quote::white_state::Head, Box<Entry>)> {
    let h = super::super::quote::white_state::head(ctx, &format!("{number:#x}")).await?;
    let state = market(
        ctx,
        e.family,
        e.pool.reference.address(),
        (e.coin, e.usdt),
        &h.selector(),
    )
    .await?;
    let mut entry = Box::new(e.clone());
    entry.pool.state = PoolState::Temple(Box::new(state));
    entry.pool.block = number;
    Ok((h, entry))
}
pub(in crate::venues::bsc_dex) fn after_probe(
    ctx: &Ctx,
    e: &Entry,
    input: Address,
    amount: U256,
    logs: &[super::super::check::simulate::SimLog],
) -> GatewayResult<Option<Box<Entry>>> {
    use alloy::sol_types::SolEvent;
    use alloy_primitives::LogData;
    let PoolState::Temple(s) = &e.pool.state else {
        return Ok(None);
    };
    let out = opposite(e, input).ok_or_else(|| fail(ctx))?;
    let (_, mut next) =
        super::super::math::temple::paid(s, input, out, amount).ok_or_else(|| fail(ctx))?;
    if let Some(log) = logs.iter().rev().find(|l| {
        l.address == s.pair
            && l.topics.first() == Some(&super::abi_v2v3::IV2Pair::Sync::SIGNATURE_HASH)
    }) {
        let data = LogData::new(log.topics.clone(), log.data.clone()).ok_or_else(|| fail(ctx))?;
        let synced =
            super::abi_v2v3::IV2Pair::Sync::decode_log_data(&data).map_err(|_| fail(ctx))?;
        next.reserves = [U256::from(synced.reserve0), U256::from(synced.reserve1)];
        next.balances = next.reserves;
    } else if !defended(e, &[e], logs) {
        return Err(fail(ctx));
    }
    let mut entry = Box::new(e.clone());
    entry.pool.state = PoolState::Temple(Box::new(next));
    Ok(Some(entry))
}

#[cfg(test)]
#[path = "temple_tests.rs"]
mod tests;
