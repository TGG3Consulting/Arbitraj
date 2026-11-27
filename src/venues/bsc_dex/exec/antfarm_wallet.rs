//! Single-transaction Antfarm fee acquisition with a bounded ERC20 debit. Persist the original instruction for recovery.
use super::super::{
    ctx::Ctx,
    pools::{antfarm_book, registry::Entry},
    quote::{v2_wallet_sim, white_state},
    send::track::Receipt,
    wallet::approve,
};
use super::{
    chain::Step,
    fill,
    plan::{DirectCall, Order, Plan},
    signed_call,
};
use crate::{
    core::types::{AntfarmBook, PoolState},
    gateway::errors::{ErrorCode, GatewayResult},
};
use alloy::{rpc::types::Log, sol, sol_types::SolCall};
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

sol! {
    struct Swap {
        address input; address output; uint16 tier; uint16 fundingTier; bool supporting;
        uint256 routerQuantity; uint256 maxDebit; uint256 minimum; uint256 maxRouterFee;
        uint256 feeFunding; uint256 maxFundingDebit; uint256 deadline;
    }
    function execute(Swap);
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Terms {
    pub module: Address,
    pub input: Address,
    pub output: Address,
    pub tier: u16,
    pub funding: u16,
    pub quantity: U256,
    pub budget: U256,
    pub fee: U256,
    pub acquire: U256,
    pub funding_budget: U256,
}
impl Terms {
    pub fn from_budget(
        book: &AntfarmBook,
        module: Address,
        input: Address,
        output: Address,
        budget: U256,
    ) -> Option<Self> {
        if input == output || !book.market.tokens.contains(&output) {
            return None;
        }
        let flow = book.route(input, true, budget)?;
        if flow.debit()? != budget {
            return None;
        }
        Some(Self {
            module,
            input,
            output,
            tier: book.market.tier,
            funding: flow.funding_tier,
            quantity: flow.primary.parameter,
            budget,
            fee: flow.primary.fee_limit,
            acquire: flow.primary.separate_atf,
            funding_budget: flow.acquisition.as_ref().map_or(U256::ZERO, |f| f.debit),
        })
    }
    pub fn data(&self, minimum: U256, deadline: U256) -> Bytes {
        executeCall {
            _0: Swap {
                input: self.input,
                output: self.output,
                tier: self.tier,
                fundingTier: self.funding,
                supporting: false,
                routerQuantity: self.quantity,
                maxDebit: self.budget,
                minimum,
                maxRouterFee: self.fee,
                feeFunding: self.acquire,
                maxFundingDebit: self.funding_budget,
                deadline,
            },
        }
        .abi_encode()
        .into()
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proof {
    pub terms: Terms,
    pub payer: Address,
    pub minimum: U256,
    pub deadline: U256,
}
impl Proof {
    pub fn data(&self) -> Bytes {
        self.terms.data(self.minimum, self.deadline)
    }
    pub(crate) async fn included(&self, ctx: &Ctx, hash: B256, r: &Receipt) -> GatewayResult<()> {
        signed_call::included(
            ctx,
            hash,
            r,
            self.payer,
            self.terms.module,
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
            self.terms.input,
            self.terms.output,
        )
    }
    pub(super) fn amounts(
        &self,
        logs: &[Log],
        wallet: Address,
        input: Address,
        output: Address,
    ) -> Option<(U256, U256)> {
        let t = &self.terms;
        if (wallet, input, output) != (self.payer, t.input, t.output)
            || self.minimum.is_zero()
            || t.budget.is_zero()
            || wallet.is_zero()
            || wallet == t.module
            || input == output
        {
            return None;
        }
        let topic = keccak256("Settled(address,address,address,uint256,uint256,uint256,uint256)");
        let topics = [
            topic,
            wallet.into_word(),
            input.into_word(),
            output.into_word(),
        ];
        let mut matching = logs
            .iter()
            .filter(|l| l.address() == t.module && l.topics() == topics);
        let data = &matching.next()?.data().data;
        if matching.next().is_some() || data.len() != 128 {
            return None;
        }
        let word = |i: usize| U256::from_be_slice(&data[32 * i..32 * (i + 1)]);
        let spent = fill::moved_logs(logs, input, Some(wallet), None)
            .checked_sub(fill::moved_logs(logs, input, None, Some(wallet)))?;
        let got = fill::moved_logs(logs, output, None, Some(wallet))
            .checked_sub(fill::moved_logs(logs, output, Some(wallet), None))?;
        // In this instruction a separate fee is purchased inside the bounded input or embedded in the swap.
        // The wallet may receive a refund in ATF. It cannot be charged an extra ATF balance.
        (spent > U256::ZERO
            && spent <= t.budget
            && got >= self.minimum
            && word(0) == spent
            && word(1) == got
            && word(2).is_zero())
        .then_some((spent, got))
    }
}
pub(in crate::venues::bsc_dex) fn enabled(e: &Entry) -> bool {
    matches!(e.pool.state, PoolState::Antfarm(_))
}
fn bad(ctx: &Ctx) -> crate::gateway::errors::GatewayError {
    ctx.err(
        ErrorCode::NoRoute,
        "Antfarm: не подтверждены модуль, выплата либо бюджет основной сделки и комиссии",
    )
}
pub(in crate::venues::bsc_dex) async fn fresh(
    ctx: &Ctx,
    e: &mut Entry,
    h: white_state::Head,
) -> GatewayResult<()> {
    antfarm_book::module(ctx, e.family, &h.selector()).await?;
    let s = antfarm_book::read(
        ctx,
        e.family,
        e.pool.reference.address(),
        (e.coin, e.usdt),
        &h.selector(),
    )
    .await?;
    e.pool.state = PoolState::Antfarm(Box::new(s));
    Ok(())
}
pub(super) async fn plan(
    ctx: &Ctx,
    step: &Step,
    payer: Address,
    height: u64,
    selected: Option<Address>,
) -> GatewayResult<Plan> {
    if !step.from_wallet || !step.to_wallet || !enabled(&step.entry) {
        return Err(bad(ctx));
    }
    let generation = ctx.settings.generation();
    let mut e = step.entry.clone();
    let h = white_state::head(ctx, &format!("{height:#x}")).await?;
    fresh(ctx, &mut e, h).await?;
    let PoolState::Antfarm(book) = &e.pool.state else {
        return Err(bad(ctx));
    };
    let module = ctx.dex_addr("dex.router", e.family.id)?;
    if selected != Some(module) {
        return Err(bad(ctx));
    }
    let o = &step.order;
    let terms = Terms::from_budget(book, module, o.token_in, o.token_out, o.amount_in)
        .ok_or_else(|| bad(ctx))?;
    let deadline = U256::from(
        h.timestamp
            .to::<u64>()
            .checked_add(ctx.int("dex.deadline_s")?)
            .ok_or_else(|| bad(ctx))?,
    );
    let proof = Proof {
        terms: terms.clone(),
        payer,
        minimum: o.min_out,
        deadline,
    };
    let measure = v2_wallet_sim::Order {
        target: module,
        payer,
        input: o.token_in,
        output: o.token_out,
        amount: o.amount_in,
        minimum: o.min_out,
        wire: v2_wallet_sim::Wire::Antfarm(Arc::new(terms)),
    };
    v2_wallet_sim::measure(ctx, h, measure, true).await?;
    let approvals = approve::fork_missing(ctx, o.token_in, payer, module, o.amount_in).await?;
    if generation != ctx.settings.generation() {
        return Err(bad(ctx));
    }
    let mut plan = Plan::new(
        ctx,
        (approvals, module, payer),
        o.token_out,
        vec![],
        o.min_out,
        deadline,
    )?;
    plan.set_direct_market(DirectCall::Antfarm(Arc::new(proof)));
    Ok(plan)
}
pub(in crate::venues::bsc_dex) fn probe(
    ctx: &Ctx,
    e: &Entry,
    o: &Order,
    now: u64,
) -> GatewayResult<(Vec<approve::Approval>, Address, Bytes)> {
    let PoolState::Antfarm(book) = &e.pool.state else {
        return Err(bad(ctx));
    };
    let module = ctx.dex_addr("dex.router", e.family.id)?;
    let t = Terms::from_budget(book, module, o.token_in, o.token_out, o.amount_in)
        .ok_or_else(|| bad(ctx))?;
    let deadline = U256::from(
        now.checked_add(ctx.int("dex.deadline_s")?)
            .ok_or_else(|| bad(ctx))?,
    );
    Ok((
        approve::fork_all(ctx, o.token_in, module, o.amount_in)?,
        module,
        t.data(o.min_out, deadline),
    ))
}
