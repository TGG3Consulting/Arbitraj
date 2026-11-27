//! Deli constant-product использует собственные резервы и обычный ввод ERC-20
//! в Uniswap V4. PoolId восстанавливается из ключа Initialize даже без позиции в PositionManager.
use super::{
    abi_v4::IStateView, discover::Found, discover_v4, families::Family,
    key::PoolKey, deli_keys::{configured,bindings}, nami, nami_batch as batch, verify::Candidate,
};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::venues::bsc_dex::ctx::Ctx;
use alloy_primitives::{Address, B256, U256, keccak256};
use serde_json::{Value,json};

alloy::sol! {
    interface IDeli {
        function poolManager() external view returns(address);
        function poolInitialized(bytes32 id) external view returns(bool);
        function getReserves(bytes32 id) external view returns(uint128 r0, uint128 r1);
        function feeProcessor() external view returns(address);
        function incentiveGauge() external view returns(address);
        function dailyEpochGauge() external view returns(address);
    }
}

pub(super) fn invalid(ctx:&Ctx) -> crate::gateway::errors::GatewayError {
    ctx.err(ErrorCode::QuoteMismatch,"Deli: ключ, роли или код хука расходятся с проверяемым рынком")
}

pub(in crate::venues::bsc_dex) async fn market(ctx:&Ctx,f:&Family,manager:Address,id:B256,pair:(Address,Address),at:&Value)
    -> GatewayResult<(PoolKey,U256,U256)> {
    let generation=ctx.settings.generation();
    let pk=configured(ctx,f,id)?;
    let PoolKey::Uniswap(k)=&pk else{return Err(invalid(ctx));};
    let roles=bindings(ctx,f)?;
    if (k.currency0,k.currency1)!=pair || k.hooks!=roles[0].0 || ctx.dex_addr("dex.pool_manager",f.id)?!=manager {
        return Err(invalid(ctx));
    }
    let hook=k.hooks;
    let mgr=IDeli::poolManagerCall{};
    let enabled=IDeli::poolInitializedCall{id};
    let reserves=IDeli::getReservesCall{id};
    let slot=IStateView::getSlot0Call{poolId:id};
    let processor=IDeli::feeProcessorCall{};
    let incentive=IDeli::incentiveGaugeCall{};
    let daily=IDeli::dailyEpochGaugeCall{};
    let vec=[batch::view(at,hook,mgr.clone()),batch::view(at,hook,enabled.clone()),batch::view(at,hook,reserves.clone()),
             batch::view(at,ctx.dex_addr("dex.state_view",f.id)?,slot.clone()),batch::view(at,hook,processor.clone()),
             batch::view(at,hook,incentive.clone()),batch::view(at,hook,daily.clone())];
    let code_calls=roles.map(|(a,_)|("eth_getCode",json!([a,at])));
    let ([m,e,r,s,p,i,d],codes)=tokio::try_join!(batch::read(ctx,vec),batch::read(ctx,code_calls))?;
    if ctx.settings.generation()!=generation || batch::decode(&m,mgr)?!=manager || !batch::decode(&e,enabled)?
        || batch::decode(&s,slot)?.sqrtPriceX96.is_zero() || batch::decode(&p,processor)?!=roles[1].0
        || batch::decode(&i,incentive)?!=roles[2].0 || batch::decode(&d,daily)?!=roles[3].0
        || !codes.iter().zip(roles).all(|(b,(_,hash))|!b.is_empty() && keccak256(b)==hash) {return Err(invalid(ctx));}
    let state=batch::decode(&r,reserves)?;
    Ok((pk,U256::from(state.r0),U256::from(state.r1)))
}

pub(super) async fn verify(ctx:&Ctx,c:&Candidate)->GatewayResult<Found> {
    let id=c.pool.parse().map_err(|_|invalid(ctx))?;
    let manager=ctx.dex_addr("dex.pool_manager",c.family.id)?;
    let at=nami::block(ctx,"latest").await?;
    let pair=super::discover::sorted(c.coin,c.quote);
    let (pk,_,_)=market(ctx,c.family,manager,id,pair,&at).await?;
    let PoolKey::Uniswap(k)=&pk else{return Err(invalid(ctx));};
    let spacing=k.tick_spacing.as_i32();
    Ok(discover_v4::from_key(c.family,manager,id,pk,Some(spacing),None))
}
