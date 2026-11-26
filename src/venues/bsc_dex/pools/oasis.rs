//! Связывает выбранную фабрику, CREATE2, кэш комиссий и контроллер с одним canonical state.
pub(crate) use super::oasis_state::OasisMarket;
pub use super::oasis_state::after;
pub(super) use super::oasis_state::load;
use super::{
    direct_module,
    families::Family,
    nami_batch as batch,
    oasis_abi::{IOasis, hash, rebate},
};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::ctx::Ctx,
};
use alloy_primitives::{Address, U256, keccak256};
use serde_json::{Value, json};
pub(crate) async fn at(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    tokens: (Address, Address),
    tag: &Value,
    controller: Option<Address>,
    adapter: bool,
) -> GatewayResult<OasisMarket> {
    let generation = ctx.settings.generation();
    let wrong = || {
        ctx.err(ErrorCode::NotApplicable,"OasisSwap: не подтверждены рынок, фабрика, версия контракта или режим внешнего MEV-контроллера")
    };
    let factory = ctx.dex_addr("dex.factory", f.id)?;
    let factory_hash = hash(ctx, f.id, "dex.factory_runtime_hash")?;
    let slot = ctx.dex_int("dex.oasis_mev_slot", f.id)?;
    if tokens.0 >= tokens.1 || factory.is_zero() || pool.is_zero() {
        return Err(wrong());
    }
    let pair = IOasis::getPairCall {
        _0: tokens.0,
        _1: tokens.1,
    };
    let bal = IOasis::balanceOfCall { _0: pool };
    let [
        code,
        mev,
        pf,
        listed,
        creation,
        reserves,
        tier,
        proportion,
        divisor,
        cache0,
        cache1,
        b0,
        b1,
        estimator,
    ] = batch::read(
        ctx,
        [
            ("eth_getCode", json!([factory, tag])),
            (
                "eth_getStorageAt",
                json!([factory, format!("{slot:#x}"), tag]),
            ),
            batch::view(tag, pool, IOasis::factoryCall {}),
            batch::view(tag, factory, pair.clone()),
            batch::view(tag, factory, IOasis::pairCodeHashCall {}),
            batch::view(tag, pool, IOasis::getReservesCall {}),
            batch::view(tag, pool, IOasis::feeCall {}),
            batch::view(tag, pool, IOasis::oasisFeeProportionCall {}),
            batch::view(tag, pool, IOasis::FEE_DIVISORCall {}),
            batch::view(tag, pool, IOasis::feeCache0Call {}),
            batch::view(tag, pool, IOasis::feeCache1Call {}),
            batch::view(tag, tokens.0, bal.clone()),
            batch::view(tag, tokens.1, bal.clone()),
            batch::view(tag, factory, IOasis::rebateEstimatorCall {}),
        ],
    )
    .await?;
    let init = batch::decode(&creation, IOasis::pairCodeHashCall {})?;
    let accepted = super::verify_algebra_dir::hashes_for(ctx, f.id)
        .map_err(|e| ctx.err(ErrorCode::NotApplicable, e))?;
    if code.is_empty()
        || keccak256(&code) != factory_hash
        || mev.as_ref() != [0u8; 32]
        || batch::decode(&pf, IOasis::factoryCall {})? != factory
        || batch::decode(&listed, pair)? != pool
        || !accepted.contains(&init)
        || super::verify::v2_create2(factory, tokens.0, tokens.1, init) != pool
    {
        return Err(wrong());
    }
    let r = batch::decode(&reserves, IOasis::getReservesCall {})?;
    let r0 = U256::from(r.r0);
    let r1 = U256::from(r.r1);
    let held0 = batch::decode(&b0, bal.clone())?
        .checked_sub(batch::decode(&cache0, IOasis::feeCache0Call {})?)
        .ok_or_else(wrong)?;
    let held1 = batch::decode(&b1, bal)?
        .checked_sub(batch::decode(&cache1, IOasis::feeCache1Call {})?)
        .ok_or_else(wrong)?;
    let rate = batch::decode(&tier, IOasis::feeCall {})?;
    let share = batch::decode(&proportion, IOasis::oasisFeeProportionCall {})?;
    if held0 < r0
        || held1 < r1
        || rate >= 10000
        || share > 10000
        || batch::decode(&divisor, IOasis::FEE_DIVISORCall {})? != 10000
    {
        return Err(wrong());
    }
    let estimator = batch::decode(&estimator, IOasis::rebateEstimatorCall {})?;
    let rate = u32::try_from(rate).map_err(|_| wrong())?;
    let (native, active_adapter, approved) = if adapter {
        let module = ctx.dex_addr("dex.router", f.id)?;
        let [bound] = batch::read(ctx, [batch::view(tag, module, IOasis::factoryCall {})]).await?;
        if batch::decode(&bound, IOasis::factoryCall {})? != factory {
            return Err(wrong());
        }
        direct_module::verify_code(ctx, f.id, module, tag).await?;
        (Address::ZERO, Some(module), false)
    } else {
        let native = ctx.dex_addr("dex.quoter", f.id)?;
        let placed = hash(ctx, f.id, "dex.router_runtime_hash")?;
        let whitelisted = IOasis::isRebateApprovedRouterCall { _0: native };
        let [native_code, bound, wrapped, allow] = batch::read(
            ctx,
            [
                ("eth_getCode", json!([native, tag])),
                batch::view(tag, native, IOasis::factoryCall {}),
                batch::view(tag, native, IOasis::WETHCall {}),
                batch::view(tag, factory, whitelisted.clone()),
            ],
        )
        .await?;
        if native_code.is_empty()
            || keccak256(&native_code) != placed
            || batch::decode(&bound, IOasis::factoryCall {})? != factory
            || batch::decode(&wrapped, IOasis::WETHCall {})? != ctx.addr("dex.native_wrapped")?
        {
            return Err(wrong());
        }
        (native, None, batch::decode(&allow, whitelisted)?)
    };
    let discounted = if estimator.is_zero() {
        Some(rate)
    } else if let Some(owner) = controller {
        rebate(ctx, factory, pool, tag, owner, rate).await?
    } else {
        None
    };
    if generation != ctx.settings.generation() {
        return Err(ctx.err(
            ErrorCode::StaleData,
            "OasisSwap: профиль изменился во время проверки",
        ));
    }
    Ok(OasisMarket {
        reserve0: r0,
        reserve1: r1,
        rate,
        rebated: discounted.unwrap_or(rate),
        share: u32::try_from(share).map_err(|_| wrong())?,
        native,
        adapter: active_adapter,
        native_rebate: approved,
        rebate_available: discounted.is_some(),
        repeatable: adapter && estimator.is_zero() && held0 == r0 && held1 == r1,
    })
}
