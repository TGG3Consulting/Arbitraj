//! Закреплённые за одним каноническим блоком обеспечение, долг и роли пары Empire.
use super::{
    abi_check::IPoolImmutables,
    abi_v2v3::{IV2Factory, IV2Pair},
    families::Family,
    family_variant::Variant,
    nami_batch as batch,
    registry::Entry,
};
use crate::{
    core::types::{EmpireState, PoolState},
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, node::abi::IERC20},
};
use alloy_primitives::{Address, U256};
use serde_json::Value;

alloy::sol! { interface IEmpire {
    function sweepableToken() external view returns(address);
    function sweptAmount() external view returns(uint256);
}}

pub(super) async fn read(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    tokens: (Address, Address),
    at: &Value,
) -> GatewayResult<EmpireState> {
    let id = f.id;
    let gen_at_start = ctx.settings.generation();
    let factory = ctx.dex_addr("dex.factory", id)?;
    let failure = || {
        ctx.err(
            ErrorCode::QuoteMismatch,
            "Empire: не совпали зарегистрированная пара, ставка или обеспечиваемый токен",
        )
    };
    let lookup = IV2Factory::getPairCall {
        tokenA: tokens.0,
        tokenB: tokens.1,
    };
    let owner = IPoolImmutables::factoryCall {};
    let reserves = IV2Pair::getReservesCall {};
    let bal = IERC20::balanceOfCall { owner: pool };
    let asset = IEmpire::sweepableTokenCall {};
    let owed = IEmpire::sweptAmountCall {};
    let f_hash = super::v2_pair_binding::expected(ctx, id, "dex.factory_runtime_hash")?;
    let init = super::verify_algebra_dir::hashes_for(ctx, id).map_err(|_| failure())?;
    if !init
        .iter()
        .any(|k| super::verify::v2_create2(factory, tokens.0, tokens.1, *k) == pool)
    {
        return Err(failure());
    }
    let ops = [
        batch::view(at, factory, lookup.clone()),
        batch::view(at, pool, owner.clone()),
        batch::view(at, pool, reserves.clone()),
        batch::view(at, tokens.0, bal.clone()),
        batch::view(at, tokens.1, bal.clone()),
        batch::view(at, pool, asset.clone()),
        batch::view(at, pool, owed.clone()),
    ];
    let ((), [listed, creator, rs, bal0, bal1, side, value]) = tokio::try_join!(
        super::solidly_classic_factory::plain(ctx, factory, f_hash, at),
        batch::read(ctx, ops)
    )?;
    if tokens.0 >= tokens.1
        || batch::decode(&listed, lookup)? != pool
        || batch::decode(&creator, owner)? != factory
        || ctx.dex_text("dex.v2_fee_source", id)? != "fixed"
        || ctx.dex_dec("dex.v2_fee_pct", id)? != rust_decimal::Decimal::new(3, 1)
        || gen_at_start != ctx.settings.generation()
    {
        return Err(failure());
    }
    let mut debt = [U256::ZERO; 2];
    let (side, amount) = (batch::decode(&side, asset)?, batch::decode(&value, owed)?);
    if side == tokens.0 {
        debt[0] = amount;
    } else if side == tokens.1 {
        debt[1] = amount;
    } else if !side.is_zero() || !amount.is_zero() {
        return Err(failure());
    }
    let exact_tokens = super::exact_wallet_tokens::eligible(ctx, id, tokens.0, tokens.1)
        && super::exact_wallet_tokens::check(ctx, id, tokens.0, tokens.1, at)
            .await
            .is_ok();
    if gen_at_start != ctx.settings.generation() {
        return Err(failure());
    }
    let rs = batch::decode(&rs, reserves)?;
    Ok(EmpireState {
        reserves: [rs.reserve0.to(), rs.reserve1.to()],
        balances: [
            batch::decode(&bal0, bal.clone())?,
            batch::decode(&bal1, bal)?,
        ],
        debt,
        exact_tokens,
        debt_aware: f.variant != Variant::EmpireRaw,
    })
}

pub(crate) async fn fresh(ctx: &Ctx, e: &Entry, at: &Value) -> GatewayResult<()> {
    let current = read(
        ctx,
        e.family,
        e.pool.reference.address(),
        (e.pool.token0, e.pool.token1),
        at,
    )
    .await?;
    if !matches!(&e.pool.state,PoolState::Empire(s) if s.as_ref()==&current) {
        return Err(ctx.err(
            ErrorCode::StaleData,
            "Empire: изменились резервы, обеспечивающий баланс или долг",
        ));
    }
    Ok(())
}
