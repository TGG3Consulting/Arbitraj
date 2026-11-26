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
use serde_json::json;

pub(in crate::venues::bsc_dex) fn is_native(e: &Entry) -> bool {
    e.family.variant == Variant::V2RouterCreate2
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
    let (code, frame) = tokio::try_join!(
        rpc::call::<Bytes>(ctx, "eth_getCode", json!([router, at])),
        rpc::call::<Bytes>(ctx, "eth_call", json!([{"to":to,"data":data},at]))
    )?;
    if code.is_empty()
        || keccak256(&code) != hash
        || !quoter_calls::decode_input(e, &frame, input, amount).is_some_and(|v| !v.is_zero())
    {
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
    let registry = IV2Factory::getPairCall {
        tokenA: e.pool.token0,
        tokenB: e.pool.token1,
    }
    .abi_encode()
    .into();
    let factory = IPoolImmutables::factoryCall {}.abi_encode().into();
    let (to, data) = base_native_v3::bound(ctx, e, router, registry, factory)?;
    let at = h.selector();
    let (code, frame) = tokio::try_join!(
        rpc::call::<Bytes>(ctx, "eth_getCode", json!([router, at])),
        rpc::call::<Bytes>(ctx, "eth_call", json!([{"to":to,"data":data},at]))
    )?;
    let expected = ctx.dex_addr("dex.factory", e.family.id)?;
    let bound = base_native_v3::checked_data(e.pool.reference.address(), &frame)
        .ok()
        .and_then(|v| IPoolImmutables::factoryCall::abi_decode_returns(&v).ok());
    if code.is_empty() || keccak256(code) != hash || bound != Some(expected) {
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
        let Ok((target, _)) = profile(&q.ctx, e.family.id) else {
            continue;
        };
        let execution = Execution {
            mode: RouteMode::Direct,
            target,
            wire: super::v2_wallet_sim::Wire::Router02,
        };
        if let Ok(option) = v2_execution::option(q, i, e.clone(), payer, execution, head).await {
            options.push(option);
        }
    }
    options
}
