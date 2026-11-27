//! Single-pair ERC20 wallet settlement through the immutable Temple adapter.
use super::super::{
    ctx::Ctx,
    pools::{registry::Entry, temple},
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
    core::types::{PoolState, TempleState},
    gateway::errors::{ErrorCode, GatewayResult},
};
use alloy::{rpc::types::Log, sol_types::SolCall};
use alloy_primitives::{Address, B256, Bytes, U256};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Key {
    pub module: Address,
    pub pair: Address,
    pub router: Address,
    pub temple: Address,
    pub stable: Address,
    pub defend: Address,
}
impl Key {
    pub fn new(m: &TempleState, module: Address) -> Self {
        Self {
            module,
            pair: m.pair,
            router: m.router,
            temple: m.temple,
            stable: m.stable,
            defend: m.defend,
        }
    }
    pub fn data(
        self,
        input: Address,
        output: Address,
        amount: U256,
        minimum: U256,
        recipient: Address,
        deadline: U256,
    ) -> Bytes {
        super::vista_wire::IVistaFlow::swapFromCall {
            tokenIn: input,
            tokenOut: output,
            amount,
            minimum,
            recipient,
            deadline,
        }
        .abi_encode()
        .into()
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Proof {
    pub key: Key,
    pub payer: Address,
    pub input: Address,
    pub output: Address,
    pub amount: U256,
    pub minimum: U256,
    pub deadline: U256,
}
impl Proof {
    pub fn data(&self) -> Bytes {
        self.key.data(
            self.input,
            self.output,
            self.amount,
            self.minimum,
            self.payer,
            self.deadline,
        )
    }
    pub(crate) async fn included(&self, ctx: &Ctx, hash: B256, r: &Receipt) -> GatewayResult<()> {
        signed_call::included(
            ctx,
            hash,
            r,
            self.payer,
            self.key.module,
            U256::ZERO,
            self.data(),
        )
        .await
    }
    pub(in crate::venues::bsc_dex) fn measured(
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
        let k = self.key;
        if (wallet, input, output) != (self.payer, self.input, self.output)
            || self.minimum.is_zero()
            || self.amount.is_zero()
            || wallet.is_zero()
            || [k.router, k.module, k.pair].contains(&wallet)
            || input == output
        {
            return None;
        }
        let transfer = |asset, from, to| fill::moved_logs(logs, asset, from, to);
        let debit =
            transfer(input, Some(wallet), None).checked_sub(transfer(input, None, Some(wallet)))?;
        let received = transfer(output, None, Some(wallet)).checked_sub(transfer(
            output,
            Some(wallet),
            None,
        ))?;
        if debit != self.amount
            || received < self.minimum
            || transfer(input, Some(wallet), Some(k.module)) != debit
        {
            return None;
        }
        let swap =
            if (input, output) == (k.stable, k.temple) || (input, output) == (k.temple, k.stable) {
                transfer(input, Some(k.module), Some(k.pair)) == debit
                    && transfer(output, Some(k.pair), Some(wallet)) == received
            } else {
                false
            };
        let burned = input == k.temple
            && output == k.defend
            && transfer(input, Some(k.module), Some(Address::ZERO)) == debit
            && transfer(output, Some(k.router), Some(wallet)) == received;
        (swap != burned).then_some((debit, received))
    }
}
pub(in crate::venues::bsc_dex) fn enabled(e: &Entry) -> bool {
    matches!(e.pool.state, PoolState::Temple(_)) && e.family.executor_kind.is_some()
}
fn fail(ctx: &Ctx) -> crate::gateway::errors::GatewayError {
    ctx.err(
        ErrorCode::NoRoute,
        "TempleDAO: не подтверждён исходный модуль, направление или сумма кошелька",
    )
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
    if !step.from_wallet || !step.to_wallet || !enabled(e) {
        return Err(fail(ctx));
    }
    let generation = ctx.settings.generation();
    let h = white_state::head(ctx, &format!("{height:#x}")).await?;
    let s = temple::market(
        ctx,
        e.family,
        e.pool.reference.address(),
        (e.coin, e.usdt),
        &h.selector(),
    )
    .await?;
    let module = ctx.dex_addr("dex.router", e.family.id)?;
    if selected != Some(module) {
        return Err(fail(ctx));
    }
    let key = Key::new(&s, module);
    let deadline = U256::from(
        h.timestamp
            .to::<u64>()
            .checked_add(ctx.int("dex.deadline_s")?)
            .ok_or_else(|| fail(ctx))?,
    );
    let proof = Proof {
        key,
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
        input: o.token_in,
        output: o.token_out,
        amount: o.amount_in,
        minimum: o.min_out,
        wire: v2_wallet_sim::Wire::Temple(std::sync::Arc::new(key)),
    };
    v2_wallet_sim::measure(ctx, h, order, true).await?;
    let approvals = approve::fork_missing(ctx, o.token_in, payer, module, o.amount_in).await?;
    if generation != ctx.settings.generation() {
        return Err(fail(ctx));
    }
    let mut plan = Plan::new(
        ctx,
        (approvals, module, payer),
        o.token_out,
        vec![],
        o.min_out,
        deadline,
    )?;
    plan.set_direct_market(DirectCall::Temple(std::sync::Arc::new(proof)));
    Ok(plan)
}

pub(in crate::venues::bsc_dex) fn probe(
    ctx: &Ctx,
    e: &Entry,
    o: &super::plan::Order,
    payer: Address,
    now: u64,
) -> GatewayResult<(Vec<approve::Approval>, Address, Bytes)> {
    let PoolState::Temple(s) = &e.pool.state else {
        return Err(fail(ctx));
    };
    if !enabled(e) {
        return Err(fail(ctx));
    }
    let target = ctx.dex_addr("dex.router", e.family.id)?;
    let deadline = U256::from(
        now.checked_add(ctx.int("dex.deadline_s")?)
            .ok_or_else(|| fail(ctx))?,
    );
    Ok((
        approve::fork_all(ctx, o.token_in, target, o.amount_in)?,
        target,
        Key::new(s, target).data(
            o.token_in,
            o.token_out,
            o.amount_in,
            o.min_out,
            payer,
            deadline,
        ),
    ))
}
