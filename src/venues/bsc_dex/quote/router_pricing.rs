//! Однопуловый Router02 с зарегистрированной CREATE2-парой. Определённый оператором
//! хеш кода, фактические фабрики и выплата плательщику проверяются перед разрешением.
use super::super::{
    ctx::Ctx,
    node::rpc,
    pools::{family_variant::Variant, registry::Entry},
    wallet::signer,
};
use super::{
    Quoting, quoter_calls,
    routing::Input,
    v2_execution::{self, Execution, OptionQuote},
    white_state::Head,
};
use crate::{
    core::types::RouteMode,
    gateway::errors::{ErrorCode, GatewayResult},
};
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use serde_json::{Value, json};

alloy::sol! { interface ISafeRouter {
    function getAmountOut(uint256 amountIn,uint256 reserveIn,uint256 reserveOut) external pure returns(uint256);
} }
fn safeswap_router_pips(ctx:&Ctx,e:&Entry)->GatewayResult<u32> {
    let rate=ctx.dex_int("dex.v2_router_fee_pips",e.family.id)?;
    u32::try_from(rate).ok().filter(|v|*v<1_000_000).ok_or_else(||ctx.err(ErrorCode::NotConfigured,"SafeSwap: нужна ставка роутера от 0 до 999999 миллионных"))
}

pub(in crate::venues::bsc_dex) fn is_native(e: &Entry) -> bool {
    matches!(
        e.family.variant,
        Variant::Gemswap | Variant::V2RouterCreate2 | Variant::V2DualCreate2 | Variant::SafeSwapV2 | Variant::ToonSwap | Variant::V2DualCreate2LiveFee | Variant::DiamondV2 | Variant::KewlV2
    )
}

pub(in crate::venues::bsc_dex) fn profile(ctx: &Ctx, id: &str) -> GatewayResult<(Address, B256)> {
    let router = ctx.dex_addr("dex.router", id)?;
    let hash = ctx
        .dex_text("dex.router_runtime_hash", id)?
        .parse::<B256>()
        .ok()
        .filter(|h| !h.is_zero() && *h != keccak256([]));
    if router.is_zero() {
        return Err(ctx.err(ErrorCode::InvalidAddress, "Неверный прямой Router02"));
    }
    Ok((
        router,
        hash.ok_or_else(|| ctx.err(ErrorCode::NotConfigured, "Не задан хеш кода Router02"))?,
    ))
}

async fn approved(
    ctx: &Ctx,
    e: &Entry,
    router: Address,
    hash: B256,
    at: &Value,
) -> GatewayResult<()> {
    if e.family.variant==Variant::SafeSwapV2 {
        use super::super::pools::{solidly_classic_factory,v2_pair_binding};
        use alloy::sol_types::SolCall;
        let fee=safeswap_router_pips(ctx,e)?;
        let reference=U256::from(1_000_000_000_000_000_000u64);
        let unit=U256::from(1_000_000u64);
        let query=ISafeRouter::getAmountOutCall{amountIn:unit,reserveIn:unit,reserveOut:reference};
        let ((),(),quote)=tokio::try_join!(
            solidly_classic_factory::plain(ctx,router,hash,at),
            v2_pair_binding::code(ctx,e.family.id,e.pool.reference.address(),at),
            rpc::call::<Bytes>(ctx,"eth_call",json!([{"to":router,"data":Bytes::from(query.abi_encode())},at]))
        )?;
        let wanted=super::super::math::v2::amount_out(unit,unit,reference,(unit-U256::from(fee),unit));
        if wanted!=Some(super::super::node::call::decode::<ISafeRouter::getAmountOutCall>(&quote)?) {
            return Err(ctx.err(ErrorCode::QuoteMismatch,"SafeSwap: ставка роутера не совпала с его формулой"));
        }
        return Ok(());
    }
    if e.family.variant==Variant::Gemswap {
        return Box::pin(super::super::pools::concave::checked(ctx,e,true,at)).await;
    }
    if e.family.variant == Variant::V2DualCreate2LiveFee {
        super::super::pools::v2_pair_binding::fresh(ctx,e,at).await?;
    }
    if e.family.variant == Variant::KewlV2 {
        return super::super::pools::kewl::checked(ctx, e, true, at).await;
    }
    if e.family.variant == Variant::DiamondV2 {
        tokio::try_join!(
            super::super::pools::diamond::fresh(ctx, e, at),
            super::super::pools::diamond::check(ctx, e.family.id, true, at)
        )?;
    } else {
        let code: Bytes = rpc::call(ctx, "eth_getCode", json!([router, at])).await?;
        if code.is_empty() || keccak256(code) != hash {
            return Err(ctx.err(ErrorCode::QuoteMismatch, "Изменился код Router02"));
        }
    }
    Ok(())
}

/// Запрашивает один и тот же блок для кода роутера, реестра выбранного пула и его котировки.
pub(in crate::venues::bsc_dex) async fn checked(
    ctx: &Ctx,
    e: &Entry,
    input: Address,
    amount: U256,
    h: Head,
) -> GatewayResult<()> {
    let (router, hash) = profile(ctx, e.family.id)?;
    if !is_native(e) || (input != e.pool.token0 && input != e.pool.token1) {
        return Err(ctx.err(
            ErrorCode::InvalidAddress,
            "Токен входа отсутствует в паре Router02",
        ));
    }
    let (to, data) = quoter_calls::request(ctx, e, input, amount)?;
    let at = h.selector();
    let ((), frame) = tokio::try_join!(
        approved(ctx, e, router, hash, &at),
        rpc::call::<Bytes>(ctx, "eth_call", json!([{"to":to,"data":data},at]))
    )?;
    if !quoter_calls::decode_input(e, &frame, input, amount).is_some_and(|v| !v.is_zero()) {
        return Err(ctx.err(
            ErrorCode::QuoteMismatch,
            "Не подтверждены код Router02, зарегистрированная пара или размер обмена",
        ));
    }
    Ok(())
}

/// У промежуточного шага плательщик — роутер, а служебное CONTRACT_BALANCE не является
/// суммой котировки. Проверяется топология по коду, фабрике и реестру; реальный minOut
/// всей группы измеряется уже собранным Router02-вызовом бандла.
pub(in crate::venues::bsc_dex) async fn registered(
    ctx: &Ctx,
    e: &Entry,
    h: Head,
) -> GatewayResult<()> {
    use super::super::pools::{abi_check::IPoolImmutables, abi_v2v3::IV2Factory, base_native_v3};
    use alloy::sol_types::SolCall;
    let (router, hash) = profile(ctx, e.family.id)?;
    if e.family.variant==Variant::Gemswap {
        return Box::pin(super::super::pools::concave::checked(ctx,e,true,&h.selector())).await;
    }
    if e.family.variant == Variant::KewlV2 {
        return super::super::pools::kewl::checked(ctx, e, true, &h.selector()).await;
    }
    let registry = IV2Factory::getPairCall {
        tokenA: e.pool.token0,
        tokenB: e.pool.token1,
    }
    .abi_encode()
    .into();
    let factory = IPoolImmutables::factoryCall {}.abi_encode().into();
    let (to, data) = base_native_v3::bound(ctx, e, router, registry, factory)?;
    let at = h.selector();
    let ((), frame) = tokio::try_join!(
        approved(ctx, e, router, hash, &at),
        rpc::call::<Bytes>(ctx, "eth_call", json!([{"to":to,"data":data},at]))
    )?;
    let expected = ctx.dex_addr("dex.factory", e.family.id)?;
    let bound = base_native_v3::checked_data(e.pool.reference.address(), &frame)
        .ok()
        .and_then(|v| IPoolImmutables::factoryCall::abi_decode_returns(&v).ok());
    if bound != Some(expected) {
        return Err(ctx.err(
            ErrorCode::QuoteMismatch,
            "Изменились код или регистрация Router02",
        ));
    }
    Ok(())
}

/// Индикативный расчёт использует состояние пула; финальная котировка измеряет оба баланса и газ.
pub(super) async fn candidates(q: &Quoting, i: &Input, head: Option<Head>) -> Vec<OptionQuote> {
    let Ok(payer) = signer::address(&q.ctx) else {
        return Vec::new();
    };
    let mut options = Vec::new();
    for e in i.entries.iter().filter(|e| is_native(e)) {
        let mars=super::super::exec::mars_plan::applicable(&q.ctx,e);
        if mars
            && let Some(t)=super::super::exec::mars_plan::candidate(&q.ctx,e,head).await {
            // Родной роутер передаёт в библиотеку нулевую процентную ставку при полной оплате ETH.
            let mut reduced=e.clone();reduced.pool.fee_pips=Some(0);
            let exec=Execution{mode:RouteMode::Direct,target:t.router,wire:super::v2_wallet_sim::Wire::NativeFlat(t)};
            if let Ok(item)=v2_execution::option(q,i,reduced,payer,exec,head).await {options.push(item)}
        }
        let native_fee=if e.family.variant==Variant::SafeSwapV2 {safeswap_router_pips(&q.ctx,e).ok()}else{e.pool.fee_pips};
        let zero_flat=mars && head.is_none() && super::super::exec::mars_plan::observed(&q.ctx,e)==Some(U256::ZERO);
        if !zero_flat && let Ok((target, _)) = profile(&q.ctx, e.family.id)
            && (e.family.variant!=Variant::SafeSwapV2 || native_fee.is_some()) {
            let mut native=e.clone();
            native.pool.fee_pips=native_fee;
            let execution = Execution {
                mode: RouteMode::Direct,
                target,
                wire: super::v2_wallet_sim::Wire::Router02,
            };
            if let Ok(option) = v2_execution::option(q, i, native, payer, execution, head).await
            {
                options.push(option);
            }
        }
        if !e.family.variant.dual_v2_execution()
            || crate::gateway::quote_stage::settlement()
            || q.ctx.text("dex.router_mode").ok().as_deref() != Some("graph")
        {
            continue;
        }
        let Ok(Some(target)) = super::super::exec::executor::address(&q.ctx) else {
            continue;
        };
        let pips = if e.family.variant == Variant::KewlV2 {
            super::super::pools::kewl::PIPS
        } else if e.family.variant==Variant::SafeSwapV2 {
            let Ok((pips,_))=super::v2_pair_quote::fee(&q.ctx,e,e.coin) else {continue};
            pips
        } else if let Some(pips) = e.pool.fee_pips {
            pips
        } else {
            continue;
        };
        if let Some(h) = head.filter(|_| e.family.variant == Variant::Gemswap)
            && Box::pin(super::super::pools::concave::checked(&q.ctx,e,false,&h.selector())).await.is_err() {continue;}
        if let Some(h) = head.filter(|_| e.family.variant == Variant::V2DualCreate2LiveFee)
            && super::super::pools::v2_pair_binding::fresh(&q.ctx,e,&h.selector()).await.is_err() { continue; }
        if let Some(h) = head.filter(|_| e.family.variant.diamond_router()) {
            let at = h.selector();
            let ok = if e.family.variant == Variant::KewlV2 {
                super::super::pools::kewl::checked(&q.ctx, e, false, &at).await
            } else {
                super::super::pools::diamond::fresh(&q.ctx, e, &at).await
            };
            if ok.is_err() {
                continue;
            }
        }
        use alloy::sol_types::SolValue;
        let step = super::v2_wallet_sim::V5Step {
            native: None,
            kind: 0,
            pool: e.pool.reference.address(),
            data: Bytes::from(pips.abi_encode()),
        };
        let execution = Execution {
            mode: RouteMode::Graph,
            target,
            wire: super::v2_wallet_sim::Wire::V5(step),
        };
        if let Ok(option) = v2_execution::option(q, i, e.clone(), payer, execution, head).await {
            options.push(option);
        }
    }
    options
}
