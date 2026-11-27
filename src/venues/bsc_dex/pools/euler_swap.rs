//! Первичное происхождение EulerSwap и общие счета. В V2 внешний registry служит поиску:
//! принадлежность пула устанавливает deployedPools самой фабрики, включая незарегистрированные экземпляры.
use super::{
    families::Family,
    family_variant::Variant,
    solidly_classic_factory,
    verify::{Candidate, Verdict},
};
use crate::{
    core::types::PoolRef,
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, node::call},
};
use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, B256};
use serde_json::Value;

alloy::sol! {
    struct V1Config {
        address vault0; address vault1; address eulerAccount;
        uint112 equilibriumReserve0; uint112 equilibriumReserve1; uint256 priceX; uint256 priceY;
        uint256 concentrationX; uint256 concentrationY; uint256 fee; uint256 protocolFee; address protocolFeeRecipient;
    }
    struct V2Config {
        address supplyVault0; address supplyVault1; address borrowVault0; address borrowVault1;
        address eulerAccount; address feeRecipient;
    }
    interface IEuler {
        function getAssets() external view returns(address a,address b);
        function getParams() external view returns(V1Config);
        function getStaticParams() external view returns(V2Config);
        function poolsByPair(address a,address b) external view returns(address[]);
        function deployedPools(address pool) external view returns(bool);
        function poolByEulerAccount(address account) external view returns(address);
        function EVC() external view returns(address);
        function legacy() external view returns(address);
        function current() external view returns(address);
        function isAccountOperatorAuthorized(address account,address operator) external view returns(bool);
        function getLimits(address input,address output) external view returns(uint256 inputMax,uint256 outputMax);
        function computeQuote(address input,address output,uint256 amount,bool exactIn) external view returns(uint256);
    }
    interface IV1 {event PoolDeployed(address indexed a,address indexed b,address indexed account,address pool);}
    interface IV2 {event PoolDeployed(address indexed a,address indexed b,address indexed account,address pool,V2Config config);}
}

pub(crate) struct Backing {
    pub installed: bool,
    pub sites: Vec<Address>,
    pub payout_vaults: [[Address; 2]; 2],
}

fn bad(ctx: &Ctx) -> crate::gateway::errors::GatewayError {
    ctx.err(
        ErrorCode::NotApplicable,
        "EulerSwap: валюта, фабрика или счёт LP не подтверждены",
    )
}

pub(crate) async fn market(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    pair: (Address, Address),
    at: &Value,
) -> GatewayResult<Backing> {
    let generation = ctx.settings.generation();
    let factory = ctx.dex_addr("dex.factory", f.id)?;
    let hash = ctx
        .dex_text("dex.factory_runtime_hash", f.id)?
        .parse::<B256>()
        .map_err(|_| bad(ctx))?;
    let ((), assets, evc) = tokio::try_join!(
        solidly_classic_factory::plain(ctx, factory, hash, at),
        call::canonical(ctx, at, pool, IEuler::getAssetsCall {}),
        call::canonical(ctx, at, factory, IEuler::EVCCall {})
    )?;
    if assets.a.is_zero()
        || assets.a >= assets.b
        || (pair != (assets.a, assets.b) && pair != (assets.b, assets.a))
    {
        return Err(bad(ctx));
    }
    let (account, payout_vaults) = if f.variant == Variant::EulerV2 {
        let (s, issued) = tokio::try_join!(
            call::canonical(ctx, at, pool, IEuler::getStaticParamsCall {}),
            call::canonical(ctx, at, factory, IEuler::deployedPoolsCall { pool })
        )?;
        if !issued {
            return Err(bad(ctx));
        }
        (
            s.eulerAccount,
            [
                [s.supplyVault0, s.borrowVault0],
                [s.supplyVault1, s.borrowVault1],
            ],
        )
    } else {
        let s = call::canonical(ctx, at, pool, IEuler::getParamsCall {}).await?;
        if call::canonical(
            ctx,
            at,
            factory,
            IEuler::poolByEulerAccountCall {
                account: s.eulerAccount,
            },
        )
        .await?
            != pool
        {
            return Err(bad(ctx));
        }
        (s.eulerAccount, [[s.vault0, s.vault0], [s.vault1, s.vault1]])
    };
    let installed = call::canonical(
        ctx,
        at,
        evc,
        IEuler::isAccountOperatorAuthorizedCall {
            account,
            operator: pool,
        },
    )
    .await?;
    if generation != ctx.settings.generation() || account.is_zero() {
        return Err(bad(ctx));
    }
    let mut sites: Vec<_> = payout_vaults.into_iter().flatten().collect();
    sites.push(account);
    sites.retain(|a| !a.is_zero());
    sites.sort_unstable();
    sites.dedup();
    Ok(Backing {
        installed,
        sites,
        payout_vaults,
    })
}

pub(super) fn created(
    f: &'static Family,
    log: &alloy::rpc::types::Log,
    quote: Address,
) -> Result<Option<Candidate>, String> {
    let data = &log.inner.data;
    let (a, b, pool) = if f.variant == Variant::EulerV2 {
        let ev = IV2::PoolDeployed::decode_log_data(data).map_err(|e| e.to_string())?;
        (ev.a, ev.b, ev.pool)
    } else {
        let ev = IV1::PoolDeployed::decode_log_data(data).map_err(|e| e.to_string())?;
        (ev.a, ev.b, ev.pool)
    };
    let coin = if a == quote {
        b
    } else if b == quote {
        a
    } else {
        return Ok(None);
    };
    Ok(Some(Candidate {
        family: f,
        coin,
        quote,
        pool: format!("{pool:#x}"),
    }))
}

pub(super) async fn verify(ctx: &Ctx, c: &Candidate) -> Verdict {
    let Ok(pool) = c.pool.parse::<Address>() else {
        return Verdict::Rejected("EulerSwap: неверный адрес пула".into());
    };
    let mut found = super::verify_woo::found(c, PoolRef::Contract(pool));
    let result = async {
        let at = super::nami::block(ctx, "latest").await?;
        let pair=(found.token0,found.token1);
        let backing=market(ctx,c.family,pool,pair,&at).await?;
        found.key=super::euler_hook::canonical(ctx,c.family,pool,pair,&at).await.ok();
        Ok::<_,crate::gateway::errors::GatewayError>(backing)
    }
    .await;
    match result {
        Ok(_) => Verdict::Taken(Box::new(found)),
        Err(e) if e.code == ErrorCode::NotApplicable => Verdict::Rejected(e.message),
        Err(e) => Verdict::Failed(e.message),
    }
}
