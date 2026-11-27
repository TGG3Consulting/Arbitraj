//! Денежное поручение DAGrow SSP. Подписанные пределы и квитанция сохраняются для сверки после перезапуска.
use super::{
    chain::Step,
    fill,
    plan::{DirectCall, Plan},
    signed_call,
};
use crate::{
    core::types::PoolState,
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{
        ctx::Ctx,
        pools::{dagrow, exact_wallet_tokens, registry::Entry},
        quote::{v2_wallet_sim as sim, white_state},
        send::track::Receipt,
        wallet::approve,
    },
};
use alloy::{rpc::types::Log, sol, sol_types::SolCall};
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use serde::{Deserialize, Serialize};
sol! {interface IExchange{
    function swapExactTokensForTokens(uint256 amount,uint256 minimum,address[] path,address recipient,uint256 deadline) external;
    function swapTokensForExactTokens(uint256 desired,uint256 maximum,address[] path,address recipient,uint256 deadline) external;
    function swapFrom(address input,address output,uint256 amount,uint256 minimum,address recipient,uint256 deadline) external;
    function swapTo(address input,address output,uint256 desired,uint256 maximum,address recipient,uint256 deadline) external;
}}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Terms {
    pub pool: Address,
    pub stable: Address,
    pub target: Address,
    pub desired: Option<U256>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proof {
    pub terms: Terms,
    pub payer: Address,
    pub input: Address,
    pub output: Address,
    pub amount: U256,
    pub minimum: U256,
    pub deadline: U256,
}
impl Proof {
    pub fn data(&self) -> Bytes {
        let Self {
            terms,
            payer: recipient,
            input,
            output,
            amount,
            minimum,
            deadline,
        } = *self;
        let path = vec![input, output];
        match (terms.target == terms.pool, terms.desired) {
            (true, None) => IExchange::swapExactTokensForTokensCall {
                amount,
                minimum,
                path,
                recipient,
                deadline,
            }
            .abi_encode(),
            (true, Some(want)) => IExchange::swapTokensForExactTokensCall {
                desired: want.max(minimum),
                maximum: amount,
                path,
                recipient,
                deadline,
            }
            .abi_encode(),
            (false, None) => IExchange::swapFromCall {
                input,
                output,
                amount,
                minimum,
                recipient,
                deadline,
            }
            .abi_encode(),
            (false, Some(want)) => IExchange::swapToCall {
                input,
                output,
                desired: want.max(minimum),
                maximum: amount,
                recipient,
                deadline,
            }
            .abi_encode(),
        }
        .into()
    }
    pub(super) async fn included(&self, ctx: &Ctx, hash: B256, r: &Receipt) -> GatewayResult<()> {
        signed_call::included(
            ctx,
            hash,
            r,
            self.payer,
            self.terms.target,
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
        let t = self.terms;
        let protected = t.target != t.pool;
        if (wallet, input, output) != (self.payer, self.input, self.output)
            || wallet.is_zero()
            || wallet == t.target
            || wallet == t.pool
            || input == output
            || self.minimum.is_zero()
        {
            return None;
        }
        let flow = |a, from, to| fill::moved_logs(logs, a, from, to);
        let spent = flow(input, Some(wallet), None).checked_sub(flow(input, None, Some(wallet)))?;
        let received =
            flow(output, None, Some(wallet)).checked_sub(flow(output, Some(wallet), None))?;
        if spent.is_zero()
            || spent > self.amount
            || (t.desired.is_none() && spent != self.amount)
            || received < self.minimum.max(t.desired.unwrap_or_default())
        {
            return None;
        }
        let sender = if protected { t.target } else { wallet };
        let event = |addr: Address, name: &[u8]| {
            let hash = keccak256(name);
            let mut ev = logs
                .iter()
                .filter(|l| l.address() == addr && l.topics().first() == Some(&hash));
            let first = ev.next()?;
            ev.next().is_none().then_some(first)
        };
        let swap = event(
            t.pool,
            b"Swap(address,uint256,uint256,uint256,uint256,address)",
        )?;
        if swap.topics().get(1..) != Some(&[sender.into_word(), wallet.into_word()][..])
            || swap.data().data.len() != 128
            || flow(output, Some(t.pool), Some(wallet)) != received
            || flow(input, Some(sender), Some(t.pool)) != spent
        {
            return None;
        }
        let vals = |log: &Log| {
            log.data()
                .data
                .chunks_exact(32)
                .map(U256::from_be_slice)
                .collect::<Vec<_>>()
        };
        let amounts = if input == t.stable {
            [spent, U256::ZERO, U256::ZERO, received]
        } else {
            [U256::ZERO, spent, received, U256::ZERO]
        };
        if vals(swap) != amounts {
            return None;
        }
        if protected {
            let settled = event(
                t.target,
                b"Settled(address,address,address,address,uint256,uint256,bool)",
            )?;
            if settled.topics().get(1..)
                != Some(&[wallet.into_word(), wallet.into_word(), input.into_word()][..])
                || settled.data().data.len() != 128
                || vals(settled)
                    != [
                        U256::from_be_slice(output.as_slice()),
                        spent,
                        received,
                        U256::from(u8::from(t.desired.is_some())),
                    ]
            {
                return None;
            }
        }
        Some((spent, received))
    }
}
impl Terms {
    pub fn proof(self, o: &sim::Order, minimum: U256, deadline: U256) -> Proof {
        Proof {
            terms: self,
            payer: o.payer,
            input: o.input,
            output: o.output,
            amount: o.amount,
            minimum,
            deadline,
        }
    }
}
fn fail(ctx: &Ctx) -> crate::gateway::errors::GatewayError {
    ctx.err(
        ErrorCode::NoRoute,
        "DAGrow: исходный плательщик, рынок или границы заявки не подтверждены",
    )
}
pub(super) async fn plan(
    ctx: &Ctx,
    step: &Step,
    payer: Address,
    height: u64,
    selection: Option<(Address, Option<U256>)>,
) -> GatewayResult<Plan> {
    let generation = ctx.settings.generation();
    let e = &step.entry;
    let o = &step.order;
    let (target, desired) = selection.ok_or_else(|| fail(ctx))?;
    let head = white_state::head(ctx, &format!("{height:#x}")).await?;
    let at = head.selector();
    let s = dagrow::state(
        ctx,
        e.family,
        e.pool.reference.address(),
        Some((o.token_in, o.token_out)),
        &at,
    )
    .await?;
    let native = e.family.executor_kind.is_none();
    if !step.from_wallet
        || !step.to_wallet
        || (if native {
            (target, payer) != (s.pool, s.payer)
        } else {
            target != s.payer
        })
    {
        return Err(fail(ctx));
    }
    if native {
        exact_wallet_tokens::check_input(ctx, e.family.id, o.token_out, &at).await?;
    }
    let terms = Terms {
        pool: s.pool,
        stable: s.stable,
        target,
        desired,
    };
    let minimum = o.min_out;
    let want = desired.map(|v| v.max(minimum));
    let request = want.unwrap_or(o.amount_in);
    let model = s
        .fill(o.token_in, want.is_none(), request)
        .ok_or_else(|| fail(ctx))?;
    let official = dagrow::official(ctx, &s, o.token_in, want.is_some(), request, &at).await?;
    if official
        != if want.is_some() {
            model.amount_in
        } else {
            model.amount_out
        }
        || model.amount_in > o.amount_in
        || model.amount_out < minimum
    {
        return Err(fail(ctx));
    }
    let order = sim::Order {
        payer,
        target,
        input: o.token_in,
        output: o.token_out,
        amount: o.amount_in,
        minimum,
        wire: sim::Wire::Dagrow(terms),
    };
    let deadline = U256::from(
        head.timestamp
            .to::<u64>()
            .checked_add(ctx.int("dex.deadline_s")?)
            .ok_or_else(|| fail(ctx))?,
    );
    let proof = terms.proof(&order, minimum, deadline);
    sim::measure(ctx, head, order, true).await?;
    let approvals = if native {
        let have = dagrow::allowance(ctx, o.token_in, payer, target, &at).await?;
        approve::fork_exact(ctx, o.token_in, target, Some(have), o.amount_in)?
    } else {
        approve::fork_missing(ctx, o.token_in, payer, target, o.amount_in).await?
    };
    if generation != ctx.settings.generation() {
        return Err(fail(ctx));
    }
    let mut plan = Plan::new(
        ctx,
        (approvals, target, payer),
        o.token_out,
        vec![],
        minimum,
        deadline,
    )?;
    plan.set_direct_market(DirectCall::Dagrow(std::sync::Arc::new(proof)));
    Ok(plan)
}
pub(in crate::venues::bsc_dex) fn probe(
    ctx: &Ctx,
    e: &Entry,
    o: &super::plan::Order,
    wallet: Address,
    now: u64,
) -> GatewayResult<(Vec<approve::Approval>, Address, Bytes)> {
    let PoolState::Dagrow(s) = &e.pool.state else {
        return Err(fail(ctx));
    };
    let target = if e.family.executor_kind.is_some() {
        s.payer
    } else {
        s.pool
    };
    if e.family.executor_kind.is_none() && s.payer != wallet {
        return Err(fail(ctx));
    }
    let terms = Terms {
        pool: s.pool,
        stable: s.stable,
        target,
        desired: None,
    };
    let proof = Proof {
        terms,
        payer: wallet,
        input: o.token_in,
        output: o.token_out,
        amount: o.amount_in,
        minimum: o.min_out,
        deadline: U256::from(now.saturating_add(ctx.int("dex.deadline_s")?)),
    };
    Ok((
        approve::fork_exact(ctx, o.token_in, target, None, o.amount_in)?,
        target,
        proof.data(),
    ))
}
