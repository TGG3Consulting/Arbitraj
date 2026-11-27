//! Кошельковое поручение Pendle со сроком, точным списанием и фактической выплатой последнего актива.
use super::super::{
    ctx::Ctx,
    pools::{pendle, pendle_module, registry::Entry},
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
use crate::gateway::errors::GatewayResult;
use alloy::{
    rpc::types::Log,
    sol,
    sol_types::{SolCall, SolEvent},
};
use alloy_primitives::{Address, B256, Bytes, U256};
use serde::{Deserialize, Serialize};
sol! {interface IAdapter {
    function execute(address market,address input,address output,uint256 amount,uint256 minimum,uint256 deadline) external returns(uint256);
    event Swap(address indexed caller,address indexed receiver,int256 netPtOut,int256 netSyOut,uint256 netSyFee,uint256 netSyToReserve);
    event Settled(address indexed payer,address indexed recipient,address indexed market,address input,address output,uint256 spent,uint256 received);
}}
pub(crate) fn data(
    market: Address,
    input: Address,
    output: Address,
    amount: U256,
    minimum: U256,
    deadline: U256,
) -> Bytes {
    IAdapter::executeCall {
        market,
        input,
        output,
        amount,
        minimum,
        deadline,
    }
    .abi_encode()
    .into()
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Proof {
    pub market: Address,
    pub module: Address,
    pub payer: Address,
    pub input: Address,
    pub output: Address,
    pub amount: U256,
    pub minimum: U256,
    pub deadline: U256,
}
impl Proof {
    pub(in crate::venues::bsc_dex) fn order(
        market: Address,
        o: &v2_wallet_sim::Order,
        deadline: U256,
    ) -> Self {
        Self {
            market,
            module: o.target,
            payer: o.payer,
            input: o.input,
            output: o.output,
            amount: o.amount,
            minimum: o.minimum,
            deadline,
        }
    }
    pub(super) fn data(&self) -> Bytes {
        data(
            self.market,
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
    pub(crate) fn measured(
        &self,
        rows: &[super::super::check::simulate::SimLog],
    ) -> Option<(U256, U256)> {
        self.amounts(
            &signed_call::logs(rows)?,
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
            || self.amount.is_zero()
            || self.minimum.is_zero()
            || input == output
        {
            return None;
        }
        let mut trades = logs.iter().filter(|e| {
            e.address() == self.market && e.topic0() == Some(&IAdapter::Swap::SIGNATURE_HASH)
        });
        let swap = IAdapter::Swap::decode_log_data(&trades.next()?.inner.data).ok()?;
        if trades.next().is_some()
            || swap.netPtOut.is_zero()
            || swap.netSyOut.is_zero()
            || swap.netPtOut.is_negative() == swap.netSyOut.is_negative()
        {
            return None;
        }
        let mut events = logs.iter().filter(|e| {
            e.address() == self.module && e.topic0() == Some(&IAdapter::Settled::SIGNATURE_HASH)
        });
        let event = IAdapter::Settled::decode_log_data(&events.next()?.inner.data).ok()?;
        if events.next().is_some()
            || (
                event.payer,
                event.recipient,
                event.market,
                event.input,
                event.output,
            ) != (wallet, wallet, self.market, input, output)
            || event.spent != self.amount
            || event.received < self.minimum
        {
            return None;
        }
        let moved = |a, s, d| fill::moved_logs(logs, a, s, d);
        let debit =
            moved(input, Some(wallet), None).checked_sub(moved(input, None, Some(wallet)))?;
        let credit =
            moved(output, None, Some(wallet)).checked_sub(moved(output, Some(wallet), None))?;
        ((debit, credit) == (event.spent, event.received)
            && moved(input, Some(wallet), Some(self.module)) == debit)
            .then_some((debit, credit))
    }
}
pub(super) async fn plan(
    ctx: &Ctx,
    s: &Step,
    payer: Address,
    height: u64,
    selected: Option<Address>,
) -> GatewayResult<Plan> {
    let e = &s.entry;
    let o = &s.order;
    if !s.from_wallet || !s.to_wallet {
        return Err(pendle::bad(ctx));
    }
    let generation = ctx.settings.generation();
    let head = white_state::head(ctx, &format!("{height:#x}")).await?;
    let target = pendle_module::checked(ctx, e, o.token_in, head).await?;
    if selected != Some(target) {
        return Err(pendle::bad(ctx));
    }
    let market = e.pool.reference.address();
    let deadline = U256::from(
        head.timestamp
            .to::<u64>()
            .checked_add(ctx.int("dex.deadline_s")?)
            .ok_or_else(|| pendle::bad(ctx))?,
    );
    let order = v2_wallet_sim::Order {
        payer,
        target,
        input: o.token_in,
        output: o.token_out,
        amount: o.amount_in,
        minimum: o.min_out,
        wire: v2_wallet_sim::Wire::Pendle(market),
    };
    let proof = Proof::order(market, &order, deadline);
    v2_wallet_sim::measure(ctx, head, order, true).await?;
    let approvals = approve::fork_missing(ctx, o.token_in, payer, target, o.amount_in).await?;
    if generation != ctx.settings.generation() {
        return Err(pendle::bad(ctx));
    }
    let mut plan = Plan::new(
        ctx,
        (approvals, target, payer),
        o.token_out,
        vec![],
        o.min_out,
        deadline,
    )?;
    plan.set_direct_market(DirectCall::Pendle(std::sync::Arc::new(proof)));
    Ok(plan)
}
pub(crate) fn probe(
    ctx: &Ctx,
    e: &Entry,
    o: &super::plan::Order,
    now: u64,
) -> GatewayResult<(Vec<approve::Approval>, Address, Bytes)> {
    let target = ctx.dex_addr("dex.router", e.family.id)?;
    let deadline = U256::from(
        now.checked_add(ctx.int("dex.deadline_s")?)
            .ok_or_else(|| pendle::bad(ctx))?,
    );
    Ok((
        approve::fork_all(ctx, o.token_in, target, o.amount_in)?,
        target,
        data(
            e.pool.reference.address(),
            o.token_in,
            o.token_out,
            o.amount_in,
            o.min_out,
            deadline,
        ),
    ))
}
