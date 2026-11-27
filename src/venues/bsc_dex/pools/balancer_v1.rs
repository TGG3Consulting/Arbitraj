//! Original BFactory/BPool, canonical role pins, raw token accounting and BMath calls.
use super::{families::Family, solidly_classic_factory::runtime};
use crate::{
    core::types::balancer_v1::{Asset, State},
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{
        ctx::Ctx,
        node::{abi::IERC20, call, multicall, rpc},
    },
};
use alloy::{
    sol,
    sol_types::{SolCall, SolEvent},
};
use alloy_primitives::{Address, B256, Bytes, U256};
use serde_json::{Value, json};
use std::collections::HashSet;

sol! {
    interface IBPool {
        event LOG_NEW_POOL(address indexed caller,address indexed pool);
        event LOG_SWAP(address indexed caller,address indexed tokenIn,address indexed tokenOut,uint256 amountIn,uint256 amountOut);
        event LOG_JOIN(address indexed caller,address indexed token,uint256 amount);
        event LOG_EXIT(address indexed caller,address indexed token,uint256 amount);
        function isBPool(address pool) external view returns(bool);
        function getCurrentTokens() external view returns(address[]);
        function getSwapFee() external view returns(uint256);
        function isPublicSwap() external view returns(bool);
        function isFinalized() external view returns(bool);
        function getBalance(address token) external view returns(uint256);
        function getDenormalizedWeight(address token) external view returns(uint256);
        function calcOutGivenIn(uint256 x,uint256 wx,uint256 y,uint256 wy,uint256 qty,uint256 fee) external pure returns(uint256);
        function calcInGivenOut(uint256 x,uint256 wx,uint256 y,uint256 wy,uint256 qty,uint256 fee) external pure returns(uint256);
        function bind(address token,uint256 balance,uint256 denorm) external;
        function unbind(address token) external;
        function rebind(address token,uint256 balance,uint256 denorm) external;
        function gulp(address token) external;
        function setSwapFee(uint256 fee) external;
        function setPublicSwap(bool on) external;
        function finalize() external;
    }
}
pub(super) fn bad(ctx: &Ctx) -> crate::gateway::errors::GatewayError {
    ctx.err(
        ErrorCode::NotApplicable,
        "Balancer V1: фабрика, код BPool, валюта или запись пула не совпадают",
    )
}
pub(crate) async fn factory(ctx: &Ctx, f: &Family, at: &Value) -> GatewayResult<Address> {
    let address = ctx.dex_addr("dex.factory", f.id)?;
    let hash = ctx
        .dex_text("dex.factory_runtime_hash", f.id)?
        .parse::<B256>()
        .map_err(|_| bad(ctx))?;
    runtime(ctx, address, hash, at).await?;
    Ok(address)
}
pub(super) fn data<C: SolCall>(to: Address, c: C) -> (Address, Bytes) {
    (to, c.abi_encode().into())
}
async fn individual(
    ctx: &Ctx,
    calls: &[(Address, Bytes)],
    at: &Value,
) -> GatewayResult<Vec<Result<Bytes, String>>> {
    let multi = ctx.addr("dex.multicall3")?;
    let width = super::load_batch::batch(ctx)?;
    let mut got = Vec::with_capacity(calls.len());
    for group in calls.chunks(width) {
        let raw: Bytes = rpc::call(
            ctx,
            "eth_call",
            json!([{"to":multi,"data":multicall::encode(group)},at]),
        )
        .await?;
        got.extend(multicall::decode(&raw, group.len())?);
    }
    Ok(got)
}
pub(super) async fn views(
    ctx: &Ctx,
    calls: &[(Address, Bytes)],
    at: &Value,
) -> GatewayResult<Vec<Bytes>> {
    individual(ctx, calls, at)
        .await?
        .into_iter()
        .map(|r| r.map_err(|e| ctx.err(ErrorCode::BadResponse, e)))
        .collect()
}
type StateCell = std::sync::Arc<tokio::sync::OnceCell<State>>;
#[derive(PartialEq, Eq)]
struct Key {
    venue: &'static str,
    family: &'static str,
    hash: B256,
    pool: Address,
}
#[derive(Default)]
struct Snapshots {
    settings: Option<(crate::settings::store::Store, u64)>,
    rows: std::collections::VecDeque<(Key, StateCell)>,
}
static SNAPSHOTS: std::sync::LazyLock<std::sync::Mutex<Snapshots>> =
    std::sync::LazyLock::new(Default::default);

pub(crate) async fn load(ctx: &Ctx, f: &Family, pool: Address, at: &Value) -> GatewayResult<State> {
    let hash = at["blockHash"]
        .as_str()
        .and_then(|h| h.parse().ok())
        .filter(|_| at["requireCanonical"] == true)
        .ok_or_else(|| bad(ctx))?;
    let generation = ctx.settings.generation();
    let key = Key {
        venue: ctx.place.id,
        family: f.id,
        hash,
        pool,
    };
    let cell = {
        let mut cache = SNAPSHOTS.lock().unwrap_or_else(|p| p.into_inner());
        if !cache
            .settings
            .as_ref()
            .is_some_and(|(s, g)| *g == generation && s.same(&ctx.settings))
        {
            cache.rows.clear();
            cache.settings = Some((ctx.settings.clone(), generation));
        }
        if let Some((_, cell)) = cache.rows.iter().find(|(k, _)| *k == key) {
            cell.clone()
        } else {
            if cache.rows.len() >= 512 {
                cache.rows.pop_front();
            }
            let cell = StateCell::default();
            cache.rows.push_back((key, cell.clone()));
            cell
        }
    };
    let state = cell.get_or_try_init(|| read(ctx, f, pool, at)).await?;
    if ctx.settings.generation() != generation {
        return Err(ctx.err(ErrorCode::StaleData, "Изменилась конфигурация Balancer V1"));
    }
    Ok(state.clone())
}
async fn read(ctx: &Ctx, f: &Family, pool: Address, at: &Value) -> GatewayResult<State> {
    let fac = factory(ctx, f, at).await?;
    let pool_hash = ctx
        .dex_text("dex.pair_runtime_hash", f.id)?
        .parse::<B256>()
        .map_err(|_| bad(ctx))?;
    runtime(ctx, pool, pool_hash, at).await?;
    let received = views(
        ctx,
        &[
            data(fac, IBPool::isBPoolCall { pool }),
            data(pool, IBPool::getCurrentTokensCall {}),
            data(pool, IBPool::getSwapFeeCall {}),
            data(pool, IBPool::isPublicSwapCall {}),
        ],
        at,
    )
    .await?;
    if !call::decode::<IBPool::isBPoolCall>(&received[0])? {
        return Err(bad(ctx));
    }
    let tokens = call::decode::<IBPool::getCurrentTokensCall>(&received[1])?;
    let fee = call::decode::<IBPool::getSwapFeeCall>(&received[2])?;
    let public_swap = call::decode::<IBPool::isPublicSwapCall>(&received[3])?;
    if tokens.len() > 8
        || tokens.iter().any(|t| t.is_zero())
        || tokens.iter().collect::<HashSet<_>>().len() != tokens.len()
    {
        return Err(bad(ctx));
    }
    let mut reads = Vec::with_capacity(3 * tokens.len());
    for &token in &tokens {
        reads.extend([
            data(pool, IBPool::getBalanceCall { token }),
            data(pool, IBPool::getDenormalizedWeightCall { token }),
            data(token, IERC20::balanceOfCall { owner: pool }),
        ]);
    }
    let rows = individual(ctx, &reads, at).await?;
    let mut assets = Vec::with_capacity(tokens.len());
    for (token, v) in tokens.into_iter().zip(rows.chunks_exact(3)) {
        let exact_transfer = super::exact_wallet_tokens::eligible_input(ctx, f.id, token)
            && super::exact_wallet_tokens::check_input(ctx, f.id, token, at)
                .await
                .is_ok();
        let required =
            |r: &Result<Bytes, String>| r.clone().map_err(|e| ctx.err(ErrorCode::BadResponse, e));
        // A reverted third token balance does not invalidate unrelated pairs. Only a
        // decoded balance can provide spendable physical output on this asset.
        let balance = v[2]
            .as_ref()
            .ok()
            .and_then(|r| call::decode::<IERC20::balanceOfCall>(r).ok());
        assets.push(Asset {
            token,
            exact_transfer: exact_transfer && balance.is_some(),
            recorded: call::decode::<IBPool::getBalanceCall>(&required(&v[0])?)?,
            weight: call::decode::<IBPool::getDenormalizedWeightCall>(&required(&v[1])?)?,
            physical: balance.unwrap_or_default(),
        });
    }
    Ok(State {
        pool,
        fee,
        public_swap,
        assets,
    })
}
pub(crate) fn request(
    ctx: &Ctx,
    s: &State,
    input: Address,
    output: Address,
    qty: U256,
    exact_in: bool,
) -> GatewayResult<(Address, Bytes)> {
    let a = s
        .assets
        .iter()
        .find(|x| x.token == input)
        .ok_or_else(|| bad(ctx))?;
    let b = s
        .assets
        .iter()
        .find(|x| x.token == output && output != input)
        .ok_or_else(|| bad(ctx))?;
    let (x, wx, y, wy, fee) = (a.recorded, a.weight, b.recorded, b.weight, s.fee);
    Ok(if exact_in {
        data(
            s.pool,
            IBPool::calcOutGivenInCall {
                x,
                wx,
                y,
                wy,
                qty,
                fee,
            },
        )
    } else {
        data(
            s.pool,
            IBPool::calcInGivenOutCall {
                x,
                wx,
                y,
                wy,
                qty,
                fee,
            },
        )
    })
}
pub(crate) async fn official(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    pair: [Address; 2],
    qty: U256,
    exact_in: bool,
    at: &Value,
) -> GatewayResult<U256> {
    let s = load(ctx, f, pool, at).await?;
    let q = request(ctx, &s, pair[0], pair[1], qty, exact_in)?;
    let g = views(ctx, &[q], at).await?;
    checked_quote(&s,pair,qty,exact_in,&g[0]).ok_or_else(||ctx.err(ErrorCode::NoRoute,"Balancer V1: расчёт BMath не допускает обмен этого размера"))
}
pub(crate) fn checked_quote(s:&State,pair:[Address;2],qty:U256,exact_in:bool,raw:&[u8])->Option<U256>{
    let quoted=IBPool::calcOutGivenInCall::abi_decode_returns(raw).ok()?;
    let fill=s.swap(pair[0],pair[1],qty,exact_in,U256::MAX)?;
    let expected=if exact_in{fill.received}else{fill.spent};
    (quoted==expected).then_some(quoted)
}
pub(super) fn membership_topics() -> [B256; 3] {
    [
        IBPool::bindCall::SELECTOR,
        IBPool::setPublicSwapCall::SELECTOR,
        IBPool::finalizeCall::SELECTOR,
    ]
    .map(selector_topic)
}
fn selector_topic(selector: [u8; 4]) -> B256 {
    let mut padded = [0; 32];
    padded[..4].copy_from_slice(&selector);
    B256::from(padded)
}
pub(crate) fn topics() -> Vec<B256> {
    let mut out = vec![
        IBPool::LOG_SWAP::SIGNATURE_HASH,
        IBPool::LOG_JOIN::SIGNATURE_HASH,
        IBPool::LOG_EXIT::SIGNATURE_HASH,
    ];
    // Anonymous LOG_CALL indexes the ABI bytes4 left-aligned in topic[0].
    for selector in [
        IBPool::bindCall::SELECTOR,
        IBPool::unbindCall::SELECTOR,
        IBPool::rebindCall::SELECTOR,
        IBPool::gulpCall::SELECTOR,
        IBPool::setSwapFeeCall::SELECTOR,
        IBPool::setPublicSwapCall::SELECTOR,
        IBPool::finalizeCall::SELECTOR,
    ] {
        out.push(selector_topic(selector));
    }
    out
}
