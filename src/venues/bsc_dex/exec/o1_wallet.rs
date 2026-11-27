//! Оплата WETH на нативном рынке o1. Поручение и квитанция закрепляют физический PoolId.
use super::super::{
    ctx::Ctx,
    pools::{family_variant::Variant, launch_market, registry::Entry},
    quote::{v2_wallet_sim, white_state},
    send::track::Receipt,
    wallet::approve,
};
use super::{
    chain::Step,
    fill,
    plan::{DirectCall, Plan},
    signed_call,
};
use crate::{
    core::types::PoolRef,
    gateway::errors::{ErrorCode, GatewayResult},
};
use alloy::{
    rpc::types::Log,
    sol,
    sol_types::{SolCall, SolEvent},
};
use alloy_primitives::{Address, B256, Bytes, U256};
use serde::{Deserialize, Serialize};
use serde_json::Value;

sol! {interface IWallet {
    function execute(bytes32 id,address tokenIn,address tokenOut,uint256 qty,uint256 minimum,uint256 deadline) external returns(uint256);
    event Filled(address indexed payer,address indexed recipient,address indexed tokenIn,address tokenOut,uint256 paid,uint256 received,bytes32 poolId);
}}

pub(in crate::venues::bsc_dex) fn enabled(e: &Entry) -> bool {
    e.family.variant == Variant::LaunchO1Wrapped
}
fn missing(ctx: &Ctx) -> crate::gateway::errors::GatewayError {
    ctx.err(
        ErrorCode::NoRoute,
        "o1: требуется выбранный и проверенный модуль WETH с исходным PoolId",
    )
}
pub(in crate::venues::bsc_dex) fn key(ctx: &Ctx, e: &Entry) -> GatewayResult<B256> {
    match e.pool.reference {
        PoolRef::Singleton { id, .. } if enabled(e) => Ok(id),
        _ => Err(missing(ctx)),
    }
}
pub(in crate::venues::bsc_dex) async fn at(ctx: &Ctx, e: &Entry, tag: &Value) -> GatewayResult<()> {
    let PoolRef::Singleton { manager, id } = e.pool.reference else {
        return Err(missing(ctx));
    };
    launch_market::check(
        ctx,
        e.family,
        manager,
        id,
        (e.pool.token0, e.pool.token1),
        tag,
    )
    .await
}
pub(in crate::venues::bsc_dex) fn data(
    id: B256,
    input: Address,
    output: Address,
    qty: U256,
    minimum: U256,
    deadline: U256,
) -> Bytes {
    IWallet::executeCall {
        id,
        tokenIn: input,
        tokenOut: output,
        qty,
        minimum,
        deadline,
    }
    .abi_encode()
    .into()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Proof {
    pub id: B256,
    pub module: Address,
    pub payer: Address,
    pub input: Address,
    pub output: Address,
    pub amount: U256,
    pub minimum: U256,
    pub deadline: U256,
}
impl Proof {
    pub(super) fn data(&self) -> Bytes {
        data(
            self.id,
            self.input,
            self.output,
            self.amount,
            self.minimum,
            self.deadline,
        )
    }
    pub(super) async fn included(&self, ctx: &Ctx, hash: B256, r: &Receipt) -> GatewayResult<()> {
        signed_call::included(
            ctx,
            hash,
            r,
            self.payer,
            self.module,
            U256::ZERO,
            self.data(),
        )
        .await
    }
    pub(in crate::venues::bsc_dex) fn measured(
        &self,
        logs: &[super::super::check::simulate::SimLog],
    ) -> Option<(U256, U256)> {
        self.amounts(
            &signed_call::logs(logs)?,
            self.payer,
            self.input,
            self.output,
        )
    }
    pub(super) fn amounts(
        &self,
        logs: &[Log],
        wallet: Address,
        input: Address,
        output: Address,
    ) -> Option<(U256, U256)> {
        if (wallet, input, output) != (self.payer, self.input, self.output)
            || self.minimum.is_zero()
            || input == output
        {
            return None;
        }
        let mut events = logs.iter().filter(|l| {
            l.address() == self.module && l.topic0() == Some(&IWallet::Filled::SIGNATURE_HASH)
        });
        let event = IWallet::Filled::decode_log_data(&events.next()?.inner.data).ok()?;
        if events.next().is_some()
            || (
                event.payer,
                event.recipient,
                event.tokenIn,
                event.tokenOut,
                event.poolId,
            ) != (wallet, wallet, input, output, self.id)
            || event.paid != self.amount
            || event.received < self.minimum
        {
            return None;
        }
        let moved = |token, from, to| fill::moved_logs(logs, token, from, to);
        let spent =
            moved(input, Some(wallet), None).checked_sub(moved(input, None, Some(wallet)))?;
        let net =
            moved(output, None, Some(wallet)).checked_sub(moved(output, Some(wallet), None))?;
        ((spent, net) == (event.paid, event.received)).then_some((spent, net))
    }
}
pub(super) async fn plan(
    ctx: &Ctx,
    step: &Step,
    payer: Address,
    height: u64,
    selected: Option<Address>,
) -> GatewayResult<Plan> {
    let e = &step.entry;
    let o = &step.order;
    if !step.from_wallet || !step.to_wallet {
        return Err(missing(ctx));
    }
    let generation = ctx.settings.generation();
    let module = ctx.dex_addr("dex.router", e.family.id)?;
    if selected != Some(module) {
        return Err(missing(ctx));
    }
    let id = key(ctx, e)?;
    let h = white_state::head(ctx, &format!("{height:#x}")).await?;
    at(ctx, e, &h.selector()).await?;
    let deadline = U256::from(
        h.timestamp
            .to::<u64>()
            .checked_add(ctx.int("dex.deadline_s")?)
            .ok_or_else(|| missing(ctx))?,
    );
    let p = Proof {
        id,
        module,
        payer,
        input: o.token_in,
        output: o.token_out,
        amount: o.amount_in,
        minimum: o.min_out,
        deadline,
    };
    let order = v2_wallet_sim::Order {
        payer,
        target: module,
        input: p.input,
        output: p.output,
        amount: p.amount,
        minimum: p.minimum,
        wire: v2_wallet_sim::Wire::O1(id),
    };
    v2_wallet_sim::measure(ctx, h, order, true).await?;
    let approvals = approve::fork_missing(ctx, p.input, payer, module, p.amount).await?;
    if generation != ctx.settings.generation() {
        return Err(ctx.err(ErrorCode::StaleData, "Настройки o1 изменились"));
    }
    let mut plan = Plan::new(
        ctx,
        (approvals, module, payer),
        p.output,
        vec![],
        p.minimum,
        p.deadline,
    )?;
    plan.set_direct_market(DirectCall::O1(std::sync::Arc::new(p)));
    Ok(plan)
}
