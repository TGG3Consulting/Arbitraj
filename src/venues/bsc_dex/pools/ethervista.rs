//! Рынки EtherVista: нативная плата за каждую сторону, активный Router и независимое покрытие продажи ETH-резервом.
use super::{
    ethervista_registry::{self, IVista},
    families::Family,
    load::Loaded,
    nami_batch as batch,
};
use crate::{
    core::types::{EtherVistaState, PoolState},
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::ctx::Ctx,
};
use alloy_primitives::{Address, U256};
use serde_json::Value;

pub(crate) struct VistaMarket {
    pub reserve0: U256,
    pub reserve1: U256,
    pub balance0: U256,
    pub balance1: U256,
    pub wrapped: Address,
    pub buy_fee: U256,
    pub sell_fee: U256,
    pub router_weth: U256,
    pub native: Address,
    pub adapter: Option<Address>,
    pub reserve_owner: Option<Address>,
    pub fee_reserve: U256,
}
impl VistaMarket {
    pub fn state(&self) -> PoolState {
        PoolState::EtherVista(Box::new(EtherVistaState {
            reserve0: self.reserve0,
            reserve1: self.reserve1,
            balance0: self.balance0,
            balance1: self.balance1,
            wrapped: self.wrapped,
            buy_fee: self.buy_fee,
            sell_fee: self.sell_fee,
        }))
    }
}

pub(crate) async fn at(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    tokens: (Address, Address),
    tag: &Value,
    adapter: bool,
) -> GatewayResult<VistaMarket> {
    let generation = ctx.settings.generation();
    let market = ethervista_registry::deployed(ctx, f, tag).await?;
    let wrong = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "EtherVista: рынок не соответствует выбранной фабрике, обёртке или версии CREATE2",
        )
    };
    let hashes = super::verify_algebra_dir::hashes_for(ctx, f.id)
        .map_err(|e| ctx.err(ErrorCode::NotApplicable, e))?;
    if tokens.0 >= tokens.1
        || (market.wrapped != tokens.0 && market.wrapped != tokens.1)
        || !hashes.contains(&market.init)
        || super::verify::v2_create2(market.factory, tokens.0, tokens.1, market.init) != pool
    {
        return Err(wrong());
    }
    let pair = IVista::getPairCall {
        tokenA: tokens.0,
        tokenB: tokens.1,
    };
    let balance = IVista::balanceOfCall { account: pool };
    let [owner, listed, reserves, held0, held1, buy, sell] = batch::read(
        ctx,
        [
            batch::view(tag, pool, IVista::factoryCall {}),
            batch::view(tag, market.factory, pair.clone()),
            batch::view(tag, pool, IVista::getReservesCall {}),
            batch::view(tag, tokens.0, balance.clone()),
            batch::view(tag, tokens.1, balance.clone()),
            batch::view(tag, pool, IVista::buyTotalFeeCall {}),
            batch::view(tag, pool, IVista::sellTotalFeeCall {}),
        ],
    )
    .await?;
    if batch::decode(&owner, IVista::factoryCall {})? != market.factory
        || batch::decode(&listed, pair)? != pool
    {
        return Err(wrong());
    }
    let r = batch::decode(&reserves, IVista::getReservesCall {})?;
    let buy = IVista::usdcToEthCall {
        units: U256::from(batch::decode(&buy, IVista::buyTotalFeeCall {})?),
    };
    let sell = IVista::usdcToEthCall {
        units: U256::from(batch::decode(&sell, IVista::sellTotalFeeCall {})?),
    };
    let [b, s] = batch::read(
        ctx,
        [
            batch::view(tag, market.router, buy.clone()),
            batch::view(tag, market.router, sell.clone()),
        ],
    )
    .await?;
    let (module, reserve_owner, fee_reserve) = if adapter {
        let (module, reserve_owner, available) =
            super::ethervista_reserve::read(ctx, f, market, tag).await?;
        (Some(module), Some(reserve_owner), available)
    } else {
        (None, None, U256::ZERO)
    };
    if ctx.settings.generation() != generation {
        return Err(ctx.err(
            ErrorCode::StaleData,
            "EtherVista: настройки изменились при чтении рынка",
        ));
    }
    Ok(VistaMarket {
        reserve0: U256::from(r.r0),
        reserve1: U256::from(r.r1),
        balance0: batch::decode(&held0, balance.clone())?,
        balance1: batch::decode(&held1, balance)?,
        wrapped: market.wrapped,
        buy_fee: batch::decode(&b, buy)?,
        sell_fee: batch::decode(&s, sell)?,
        router_weth: market.router_weth,
        native: market.router,
        adapter: module,
        reserve_owner,
        fee_reserve,
    })
}

pub(super) async fn load(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    tokens: (Address, Address),
    block: &str,
) -> GatewayResult<Loaded> {
    let tag = super::elfomo::block(ctx, block).await?;
    let market = at(ctx, f, pool, tokens, &tag, false).await?;
    Ok(Loaded {
        state: market.state(),
        fee_pips: Some(0),
        ticks: None,
        bins: None,
    })
}
