//! Файл: venues/bsc_dex/pools/elfomo/market.rs.
//! Что делает: читает цену от фактического кошелька, отсеивает исчерпанный остаток и строит лестницу по блоку.
//! Кто вызывает: pools/load, quote/quoter. От чего зависит: ElfomoFi.getAmountOut, ERC-20 balanceOf и allowance.
//! Настройки: dex.pmm_ladder_usd и dex.pmm_ladder_counter.
use super::{IElfomo, Profile, Proto, block, verify_at};
use crate::core::types::{OraclePmmData, PoolState};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::venues::bsc_dex::{
    ctx::Ctx,
    node::{abi::IERC20, call, rpc},
    pools::{families::Family, ladder_counter, registry::Registry},
    tokens::Tokens,
    wallet::signer,
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};
use serde_json::{Value, json};

/// Край, равный всему доступному остатку, у ElfomoFi может маскировать насыщение выхода.
pub fn within_capacity(out: U256, capacity: U256) -> bool {
    !out.is_zero() && out < capacity
}

pub(super) async fn ask(
    ctx: &Ctx,
    from: Address,
    to: Address,
    dx: U256,
    tag: &Value,
    owner: Address,
    target: Address,
) -> GatewayResult<U256> {
    let data = Bytes::from(
        IElfomo::getAmountOutCall {
            fromToken: from,
            toToken: to,
            fromAmount: dx,
        }
        .abi_encode(),
    );
    let raw: Bytes = rpc::call(
        ctx,
        "eth_call",
        json!([{"from":owner,"to":target,"data":data},tag]),
    )
    .await?;
    call::decode::<IElfomo::getAmountOutCall>(&raw)
}

async fn capacity(
    ctx: &Ctx,
    profile: &Profile,
    token: Address,
    tag: &Value,
) -> GatewayResult<U256> {
    let balance = IERC20::balanceOfCall {
        owner: profile.vault,
    }
    .abi_encode();
    let allow = IERC20::allowanceCall {
        owner: profile.vault,
        spender: profile.pool,
    }
    .abi_encode();
    let balance: Bytes = rpc::call(
        ctx,
        "eth_call",
        json!([{"to":token,"data":Bytes::from(balance)},tag]),
    )
    .await?;
    let allow: Bytes = rpc::call(
        ctx,
        "eth_call",
        json!([{"to":token,"data":Bytes::from(allow)},tag]),
    )
    .await?;
    Ok(call::decode::<IERC20::balanceOfCall>(&balance)?
        .min(call::decode::<IERC20::allowanceCall>(&allow)?))
}

async fn target(ctx: &Ctx, profile: &Profile, id: &str, tag: &Value) -> GatewayResult<Address> {
    if ctx.dex_text("dex.kind", id)? == "elfomofi_module" {
        let module = ctx.dex_addr("dex.router", id)?;
        super::super::direct_module::verify_code(ctx, id, module, tag).await?;
        return Ok(module);
    }
    Ok(profile.pool)
}

pub async fn quote(
    ctx: &Ctx,
    id: &str,
    from: Address,
    to: Address,
    dx: U256,
    at: &str,
) -> GatewayResult<U256> {
    let profile = Profile::read(ctx, id, Proto::Elfomo)?;
    if !profile.unordered(from, to) {
        return Err(ctx.err(ErrorCode::NoRoute, "Пара ElfomoFi не подтверждена"));
    }
    let tag = block(ctx, at).await?;
    verify_at(ctx, &profile, &tag).await?;
    let target = target(ctx, &profile, id, &tag).await?;
    let out = ask(ctx, from, to, dx, &tag, signer::address(ctx)?, target).await?;
    if !within_capacity(out, capacity(ctx, &profile, to, &tag).await?) {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            "Нулевая котировка или исчерпан объём vault ElfomoFi",
        ));
    }
    profile.current(ctx)?;
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
pub async fn load(
    ctx: &Ctx,
    tokens: &Tokens,
    reg: &Registry,
    f: &Family,
    router: Address,
    coin: Address,
    ordered: (Address, Address),
    at: &str,
) -> GatewayResult<PoolState> {
    let tag = block(ctx, at).await?;
    let profile = Profile::read(ctx, f.id, Proto::Elfomo)?;
    verify_at(ctx, &profile, &tag).await?;
    if router != profile.pool
        || !profile.pair(coin, profile.counter)
        || !(ordered == (coin, profile.counter) || ordered == (profile.counter, coin))
    {
        return Err(ctx.err(ErrorCode::NoRoute, "Недопустимая пара ElfomoFi"));
    }
    let owner = signer::address(ctx)?;
    let target = target(ctx, &profile, f.id, &tag).await?;
    let (coin_cap, quote_cap) = (
        capacity(ctx, &profile, coin, &tag).await?,
        capacity(ctx, &profile, profile.counter, &tag).await?,
    );
    let sizes = ladder_counter::amounts(ctx, tokens, reg, f.id, profile.counter).await?;
    let reader = super::ladder::Reader {
        ctx,
        tag: &tag,
        wallet: owner,
        router: profile.pool,
        counter: profile.counter,
        target,
        coin,
    };
    let (buy, sell) = tokio::try_join!(
        reader.side(true, &sizes, coin_cap),
        reader.side(false, &sizes, quote_cap)
    )?;
    profile.current(ctx)?;
    Ok(PoolState::OraclePmm(Box::new(OraclePmmData {
        price: U256::ZERO,
        spread: U256::ZERO,
        coeff: U256::ZERO,
        feasible: !buy.is_empty() || !sell.is_empty(),
        paused: false,
        implementation: profile.implementation,
        base_is_token0: ordered.0 == coin,
        ladder_quote_in: buy,
        ladder_base_in: sell,
        shared_accounts: Vec::new(),
        pool_token0: None,
    })))
}
