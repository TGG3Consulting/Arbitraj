//! Оплаченный из WETH обмен SafeSwap. Квитанцию восстанавливаем по сохранённому модулю и ровно тем данным, которые подписал кошелёк.
use super::super::{
    ctx::Ctx,
    pools::{abi_v2v3::IV2Pair, registry::Entry, safeswap_trade},
    quote::{v2_wallet_sim, white_state},
    send::track::Receipt,
    wallet::approve,
};
use super::{
    chain::Step,
    fill,
    plan::{DirectCall, Plan},
    signed_call,
    vista_wire::IVistaFlow,
};
use crate::gateway::errors::GatewayResult;
use alloy::{
    rpc::types::Log,
    sol,
    sol_types::{SolCall, SolEvent},
};
use alloy_primitives::{Address, B256, Bytes, U256};
use serde::{Deserialize, Serialize};

sol! { interface ISettlement {
    event Filled(address indexed sender,address indexed recipient,address indexed input,address output,
                 uint256 spent,uint256 received,uint256 nativeFee,bool separateFee);
}}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Proof {
    pub pool: Address,
    pub module: Address,
    pub payer: Address,
    pub input: Address,
    pub output: Address,
    pub budget: U256,
    pub minimum: U256,
    pub deadline: U256,
}
impl Proof {
    pub(in crate::venues::bsc_dex) fn order(
        pool: Address,
        o: &v2_wallet_sim::Order,
        minimum: U256,
        deadline: U256,
    ) -> Self {
        Self {
            pool,
            module: o.target,
            payer: o.payer,
            input: o.input,
            output: o.output,
            budget: o.amount,
            minimum,
            deadline,
        }
    }
    pub(in crate::venues::bsc_dex) fn data(&self) -> Bytes {
        IVistaFlow::swapFromCall {
            tokenIn: self.input,
            tokenOut: self.output,
            amount: self.budget,
            minimum: self.minimum,
            recipient: self.payer,
            deadline: self.deadline,
        }
        .abi_encode()
        .into()
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
        if wallet.is_zero()
            || input == output
            || [self.module, self.pool].contains(&wallet)
            || self.minimum.is_zero()
            || self.budget.is_zero()
            || (wallet, input, output) != (self.payer, self.input, self.output)
        {
            return None;
        }
        let mut swaps = logs.iter().filter(|l| {
            l.address() == self.pool && l.topic0() == Some(&IV2Pair::Swap::SIGNATURE_HASH)
        });
        let swap = IV2Pair::Swap::decode_log_data(&swaps.next()?.inner.data).ok()?;
        let (incoming, outgoing, other_in, other_out) = if input < output {
            (
                swap.amount0In,
                swap.amount1Out,
                swap.amount1In,
                swap.amount0Out,
            )
        } else {
            (
                swap.amount1In,
                swap.amount0Out,
                swap.amount0In,
                swap.amount1Out,
            )
        };
        if swaps.next().is_some()
            || incoming.is_zero()
            || outgoing.is_zero()
            || !other_in.is_zero()
            || !other_out.is_zero()
        {
            return None;
        }
        let mut fills = logs.iter().filter(|l| {
            l.address() == self.module && l.topic0() == Some(&ISettlement::Filled::SIGNATURE_HASH)
        });
        let f = ISettlement::Filled::decode_log_data(&fills.next()?.inner.data).ok()?;
        if fills.next().is_some()
            || (f.sender, f.recipient, f.input, f.output) != (wallet, wallet, input, output)
            || f.separateFee
            || f.spent != self.budget
            || f.received < self.minimum
        {
            return None;
        }
        let moved = |token, from, to| fill::moved_logs(logs, token, from, to);
        let paid =
            moved(input, Some(wallet), None).checked_sub(moved(input, None, Some(wallet)))?;
        let got =
            moved(output, None, Some(wallet)).checked_sub(moved(output, Some(wallet), None))?;
        ((paid, got) == (f.spent, f.received)
            && moved(input, Some(wallet), Some(self.module)) == paid)
            .then_some((paid, got))
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
    if !s.from_wallet || !s.to_wallet {
        return Err(safeswap_trade::bad(ctx));
    }
    let generation = ctx.settings.generation();
    let head = white_state::head(ctx, &format!("{height:#x}")).await?;
    let pool = e.pool.reference.address();
    let module = Box::pin(safeswap_trade::market(
        ctx,
        e.family,
        pool,
        (e.pool.token0, e.pool.token1),
        &head.selector(),
    ))
    .await?;
    if selected != Some(module) {
        return Err(safeswap_trade::bad(ctx));
    }
    let end = U256::from(
        head.timestamp
            .to::<u64>()
            .checked_add(ctx.int("dex.deadline_s")?)
            .ok_or_else(|| safeswap_trade::bad(ctx))?,
    );
    let o = &s.order;
    let order = v2_wallet_sim::Order {
        payer,
        target: module,
        input: o.token_in,
        output: o.token_out,
        amount: o.amount_in,
        minimum: o.min_out,
        wire: v2_wallet_sim::Wire::SafeSwap(pool),
    };
    let proof = Proof::order(pool, &order, order.minimum, end);
    v2_wallet_sim::measure(ctx, head, order, true).await?;
    let approvals = approve::fork_missing(ctx, o.token_in, payer, module, o.amount_in).await?;
    if ctx.settings.generation() != generation {
        return Err(safeswap_trade::bad(ctx));
    }
    let mut plan = Plan::new(
        ctx,
        (approvals, module, payer),
        o.token_out,
        Vec::new(),
        o.min_out,
        end,
    )?;
    plan.set_direct_market(DirectCall::SafeSwap(std::sync::Arc::new(proof)));
    Ok(plan)
}
pub(in crate::venues::bsc_dex) fn probe(
    ctx: &Ctx,
    e: &Entry,
    o: &super::plan::Order,
    wallet: Address,
    now: u64,
) -> GatewayResult<(Vec<approve::Approval>, Address, Bytes)> {
    let module = ctx.dex_addr("dex.router", e.family.id)?;
    let deadline = U256::from(
        now.checked_add(ctx.int("dex.deadline_s")?)
            .ok_or_else(|| safeswap_trade::bad(ctx))?,
    );
    let p = Proof {
        pool: e.pool.reference.address(),
        module,
        payer: wallet,
        input: o.token_in,
        output: o.token_out,
        budget: o.amount_in,
        minimum: o.min_out,
        deadline,
    };
    Ok((
        approve::fork_all(ctx, o.token_in, module, o.amount_in)?,
        module,
        p.data(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    #[test]
    fn signed_wallet_orders_rebuild_from_their_archived_debits_and_credits() {
        let bytes = include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/safeswap_ethereum_protected_wallets.json.gz"
        );
        let archive: Value =
            serde_json::from_reader(flate2::read::GzDecoder::new(bytes.as_slice())).unwrap();
        let mut measured = 0;
        for case in archive["cases"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|v| v["mode"] == "module")
        {
            for side in ["buy", "sell"] {
                let leg = &case[side];
                let d = &leg["exact"];
                let tx: Bytes = serde_json::from_value(d["transaction"]["data"].clone()).unwrap();
                let call = IVistaFlow::swapFromCall::abi_decode(&tx).unwrap();
                let amount = |key: &str| d[key].as_str().unwrap().parse::<U256>().unwrap();
                let addr = |key: &str| case[key].as_str().unwrap().parse::<Address>().unwrap();
                let proof = Proof {
                    pool: addr("pool"),
                    module: addr("module"),
                    payer: addr("payer"),
                    input: call.tokenIn,
                    output: call.tokenOut,
                    budget: amount("spent"),
                    minimum: amount("received"),
                    deadline: call.deadline,
                };
                let mut restored: Proof =
                    serde_json::from_slice(&serde_json::to_vec(&proof).unwrap()).unwrap();
                let logs = serde_json::from_value::<
                    Vec<super::super::super::check::simulate::SimLog>,
                >(d["logs"].clone())
                .unwrap();
                assert_eq!(restored.data(), tx);
                assert_eq!(
                    restored.measured(&logs),
                    Some((proof.budget, proof.minimum))
                );
                restored.minimum += U256::ONE;
                assert_eq!(restored.measured(&logs), None);
                restored.minimum = proof.minimum;
                restored.pool = proof.module;
                assert_eq!(restored.measured(&logs), None);
                assert!(leg["above"].is_object() && leg["expired"].is_object());
                measured += 1;
            }
        }
        assert_eq!(measured, 8);
    }
}
