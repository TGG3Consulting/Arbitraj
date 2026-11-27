//! Штатный Router Maverick V2 принимает ERC20 либо ETH для оплаты WETH.
//! Для поручения с оплатой ERC20 допускается вход, который Router всегда берёт с подписанта.
use super::{
    abi_maverick::{IMaverickV2Factory as Factory, IMaverickV2Pool as Pool},
    exact_wallet_tokens, nami_batch as batch,
    registry::Entry,
    solidly_classic_factory::plain,
};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::ctx::Ctx,
};
use alloy::sol;
use alloy_primitives::{Address, B256};
use serde_json::Value;

sol! {
    interface Router {
        function weth() external view returns (address);
    }
}

pub(crate) async fn checked(
    ctx: &Ctx,
    e: &Entry,
    input: Address,
    output: Address,
    at: &Value,
) -> GatewayResult<Address> {
    let id = e.family.id;
    let generation = ctx.settings.generation();
    let fail = || {
        ctx.err(
            ErrorCode::NoRoute,
            "Maverick V2: изменились роль контракта, способ оплаты или состав пула",
        )
    };
    let router = ctx.dex_addr("dex.router", id)?;
    let factory = ctx.dex_addr("dex.factory", id)?;
    let hash = |key| ctx.dex_text(key, id)?.parse::<B256>().map_err(|_| fail());
    let (router_hash, factory_hash) = (
        hash("dex.router_runtime_hash")?,
        hash("dex.factory_runtime_hash")?,
    );
    let pool = e.pool.reference.address();
    let registration = Factory::isFactoryPoolCall { pool };
    let a = Pool::tokenACall {};
    let b = Pool::tokenBCall {};
    let open = Pool::permissionedSwapCall {};
    let owner = Pool::factoryCall {};
    let wrapped = Router::wethCall {};
    let (
        (),
        (),
        (),
        [
            listed,
            token_a,
            token_b,
            permissioned,
            pool_factory,
            router_factory,
            weth,
        ],
    ) = tokio::try_join!(
        plain(ctx, router, router_hash, at),
        plain(ctx, factory, factory_hash, at),
        exact_wallet_tokens::check(ctx, id, input, output, at),
        batch::read(
            ctx,
            [
                batch::view(at, factory, registration.clone()),
                batch::view(at, pool, a.clone()),
                batch::view(at, pool, b.clone()),
                batch::view(at, pool, open.clone()),
                batch::view(at, pool, owner.clone()),
                batch::view(at, router, owner.clone()),
                batch::view(at, router, wrapped.clone()),
            ]
        ),
    )?;
    if !batch::decode(&listed, registration)?
        || batch::decode(&permissioned, open)?
        || batch::decode(&token_a, a)? != input.min(output)
        || batch::decode(&token_b, b)? != input.max(output)
        || batch::decode(&pool_factory, owner.clone())? != factory
        || batch::decode(&router_factory, owner)? != factory
        || input == ctx.addr("dex.native_wrapped")?
        || batch::decode(&weth, wrapped)? != ctx.addr("dex.native_wrapped")?
        || generation != ctx.settings.generation()
    {
        return Err(fail());
    }
    Ok(router)
}
