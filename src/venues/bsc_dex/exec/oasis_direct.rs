//! Штатный OasisSwap. Параметр скидки входит в ABI, а минимум проверяется
//! в конце по кошельку получателя. Источник — верифицированный OasisSwapRouter.sol.
use super::super::{
    ctx::Ctx,
    pools::registry::Entry,
    quote::{oasis_pricing, white_state},
    wallet::{approve, signer},
};
use super::{
    chain::Step,
    plan::{DirectCall, Order, Plan},
};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::{sol, sol_types::SolCall};
use alloy_primitives::{Address, Bytes, U256};
use std::collections::HashMap;

sol! {interface IOasisRouter {
    function swapExactTokensForTokensSupportingFeeOnTransferTokens(uint256 amountIn,uint256 amountOutMin,address[] calldata path,address to,uint256 deadline,bool useRebate) external;
    function getAmountsOut(uint256 amountIn,address[] calldata path,bool useRebate,address user) external view returns(uint256[]);
}}
type Decisions = HashMap<crate::core::types::PoolRef, (Address, bool, bool)>;
tokio::task_local! {static ACTIVE:Decisions;}

/// Плательщик, признак скидки и форма исполнения зафиксированы для конкретной пары этой сделки.
pub(in crate::venues::bsc_dex) fn chosen(e: &Entry) -> Option<(Address, bool, bool)> {
    ACTIVE
        .try_with(|map| map.get(&e.pool.reference).copied())
        .ok()
        .flatten()
}
fn chosen_fee(
    option: Option<crate::core::types::RouterFeeMode>,
    adapter: bool,
    eligible: bool,
) -> Option<bool> {
    use crate::core::types::RouterFeeMode;
    match (adapter, option) {
        (true, None) => Some(eligible),
        (true, Some(_)) => None,
        (false, Some(RouterFeeMode::Rebate)) => eligible.then_some(true),
        (false, None | Some(RouterFeeMode::Standard)) => Some(false),
        (
            false,
            Some(RouterFeeMode::NativeGrossMinimum | RouterFeeMode::NativeSettlement { .. } | RouterFeeMode::ProtectedWallet{..}|RouterFeeMode::CarbonWallet{..}|RouterFeeMode::AntfarmProceeds{..}|RouterFeeMode::Dagrow{..}),
        ) => None,
    }
}
pub(super) async fn scope<F, T>(
    q: &super::super::quote::Quoting,
    route: &crate::core::types::DexRoute,
    work: F,
) -> GatewayResult<T>
where
    F: std::future::Future<Output = GatewayResult<T>>,
{
    let entries: Vec<_> = route
        .legs
        .iter()
        .flat_map(|l| &l.hops)
        .filter_map(|h| {
            q.reg
                .get(&h.pool)
                .filter(oasis_pricing::is_oasis)
                .map(|e| (e, h.fee_mode))
        })
        .collect();
    if entries.is_empty() {
        return work.await;
    }
    let adapter = route.mode == crate::core::types::RouteMode::Graph;
    if adapter && super::executor::takes(&q.ctx, &q.reg, route, false)?.is_none() {
        return Err(q.ctx.err(
            ErrorCode::NoRoute,
            "OasisSwap: для графа требуется подтверждённый V5",
        ));
    }
    let wallet = signer::address(&q.ctx)?;
    let head = white_state::head(&q.ctx, "latest").await?;
    let mut decisions = Decisions::new();
    for (e, option) in entries {
        let m = oasis_pricing::state(&q.ctx, &e, wallet, head, adapter).await?;
        let eligible = oasis_pricing::use_rebate(&m, adapter);
        let rebate = chosen_fee(option, adapter, eligible).ok_or_else(|| {
            q.ctx.err(
                ErrorCode::StaleData,
                "OasisSwap: подтверждённый выбор ставки недоступен",
            )
        })?;
        if decisions
            .insert(e.pool.reference, (wallet, rebate, adapter))
            .is_some_and(|old| old != (wallet, rebate, adapter))
        {
            return Err(q.ctx.err(
                ErrorCode::NoRoute,
                "Для одного пула выбраны несовместимые ставки",
            ));
        }
    }
    ACTIVE.scope(decisions, work).await
}
pub(in crate::venues::bsc_dex) fn swap_data(
    input: Address,
    output: Address,
    amount: U256,
    minimum: U256,
    wallet: Address,
    deadline: U256,
    rebate: bool,
) -> Bytes {
    IOasisRouter::swapExactTokensForTokensSupportingFeeOnTransferTokensCall {
        amountIn: amount,
        amountOutMin: minimum,
        path: vec![input, output],
        to: wallet,
        deadline,
        useRebate: rebate,
    }
    .abi_encode()
    .into()
}
pub(super) async fn plan(
    ctx: &Ctx,
    steps: &[Step],
    wallet: Address,
    now: u64,
    all: bool,
) -> GatewayResult<Plan> {
    let [s] = steps else {
        return Err(ctx.err(ErrorCode::NoRoute, "Прямой Oasis требует один пул"));
    };
    let generation = ctx.settings.generation();
    let head = white_state::head(ctx, "latest").await?;
    let m = oasis_pricing::state(ctx, &s.entry, wallet, head, false).await?;
    let eligible = oasis_pricing::use_rebate(&m, false);
    let rebate = match chosen(&s.entry) {
        Some((sender, expected, false)) if sender == wallet && (!expected || eligible) => expected,
        Some(_) => {
            return Err(ctx.err(
                ErrorCode::StaleData,
                "OasisSwap: изменились плательщик или право на выбранную ставку",
            ));
        }
        None => false,
    };
    let mut p = super::white_direct::build(ctx, s, wallet, m.native, now, all, generation).await?;
    p.set_direct_market(DirectCall::Oasis {
        input: s.order.token_in,
        amount: s.order.amount_in,
        rebate,
    });
    Ok(p)
}
pub(in crate::venues::bsc_dex) async fn probe(
    ctx: &Ctx,
    e: &Entry,
    o: Order,
    wallet: Address,
    now: u64,
) -> GatewayResult<(Vec<approve::Approval>, Address, Bytes)> {
    let p = plan(ctx, &super::plan::single(e, o), wallet, now, true).await?;
    let data = p.data(U256::ZERO);
    Ok((p.approvals, p.router, data))
}
#[cfg(test)]
#[path = "oasis_tests.rs"]
mod tests;
