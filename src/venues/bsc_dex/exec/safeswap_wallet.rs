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
    sol_types::{SolCall, SolEvent},
};
use alloy_primitives::{Address, B256, Bytes, U256};
use serde::{Deserialize, Serialize};

#[allow(clippy::too_many_arguments)]
mod abi {
    alloy::sol! { interface ISettlement {
        event Funded(address indexed sender,address indexed input,uint256 spent,uint256 credited);
        event Filled(address indexed sender,address indexed recipient,address indexed input,address output,
                     uint256 spent,uint256 received,uint256 nativeFee,bool separateFee);
    }}
}
use abi::ISettlement;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Proof {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub native_router: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub degen_module: bool,
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
            native_router: matches!(o.wire, v2_wallet_sim::Wire::DegenRouter(_)),
            degen_module: matches!(o.wire, v2_wallet_sim::Wire::DegenModule(_)),
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
        if self.native_router {
            return super::degen_router::data(self);
        }
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
            || (!self.degen_module && !other_in.is_zero())
            || !other_out.is_zero()
        {
            return None;
        }
        if self.native_router {
            if swap.sender != self.module || swap.to != wallet {
                return None;
            }
            let moved = |token, src, dst| fill::moved_logs(logs, token, src, dst);
            let paid =
                moved(input, Some(wallet), None).checked_sub(moved(input, None, Some(wallet)))?;
            let got =
                moved(output, None, Some(wallet)).checked_sub(moved(output, Some(wallet), None))?;
            return (paid == self.budget
                && got >= self.minimum
                && got <= outgoing
                && moved(output, Some(self.pool), Some(wallet)) == got
                && !moved(input, Some(wallet), Some(self.pool)).is_zero())
            .then_some((paid, got));
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
        if self.degen_module {
            let mut events = logs.iter().filter(|l| {
                l.address() == self.module
                    && l.topic0() == Some(&ISettlement::Funded::SIGNATURE_HASH)
            });
            if let Some(log) = events.next() {
                let debit = ISettlement::Funded::decode_log_data(&log.inner.data).ok()?;
                if events.next().is_some()
                    || debit.sender != wallet
                    || debit.input != input
                    || debit.spent != f.spent
                    || debit.credited.is_zero()
                    || debit.credited > f.spent
                    || f.nativeFee != U256::ZERO
                    || moved(input, Some(wallet), Some(self.module)).is_zero()
                    || moved(output, Some(self.pool), Some(wallet)).is_zero()
                {
                    return None;
                }
                if swap.to != wallet {
                    return None;
                }
                // На зарегистрированном Degen-пуле отражение может увеличить balanceOf выходного токена.
                // Пара регистрирует этот прирост как дополнительный вход, а Funded и Filled измеряют кошелёк.
                // Reflection-токен может записать в Transfer только чистую часть. Контракт сравнил четыре balanceOf и связал обе суммы с одним payer.
                return Some((f.spent, f.received));
            }
        }
        if !other_in.is_zero() {
            return None;
        }
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
    let direct = e.family.variant == super::super::pools::family_variant::Variant::DegenSwap
        && selected.is_some()
        && selected == ctx.dex_addr("dex.quoter", e.family.id).ok();
    let module = if direct {
        Box::pin(super::degen_router::checked(
            ctx,
            e,
            s.order.token_in,
            &head.selector(),
        ))
        .await?
    } else {
        Box::pin(safeswap_trade::market(
            ctx,
            e.family,
            pool,
            (e.pool.token0, e.pool.token1),
            &head.selector(),
        ))
        .await?
    };
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
        wire: if direct {
            v2_wallet_sim::Wire::DegenRouter(pool)
        } else if e.family.variant == super::super::pools::family_variant::Variant::DegenSwap {
            v2_wallet_sim::Wire::DegenModule(pool)
        } else {
            v2_wallet_sim::Wire::SafeSwap(pool)
        },
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
    let native_router = e.family.variant == super::super::pools::family_variant::Variant::DegenSwap
        && super::super::pools::exact_wallet_tokens::eligible_input(ctx, e.family.id, o.token_in);
    let module = ctx.dex_addr(
        if native_router {
            "dex.quoter"
        } else {
            "dex.router"
        },
        e.family.id,
    )?;
    let deadline = U256::from(
        now.checked_add(ctx.int("dex.deadline_s")?)
            .ok_or_else(|| safeswap_trade::bad(ctx))?,
    );
    let p = Proof {
        native_router,
        degen_module: e.family.variant == super::super::pools::family_variant::Variant::DegenSwap
            && !native_router,
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
    fn degen_taxed_funding_is_measured_by_the_attested_contract(){
        let archive:Value=serde_json::from_reader(flate2::read::GzDecoder::new(include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/degenswap_ethereum_funded.json.gz").as_slice())).unwrap();
        let addr=|v:&Value|v.as_str().unwrap().parse::<Address>().unwrap();
        let mut count=0;
        for r in archive["cases"].as_array().unwrap().iter().filter(|r|r["mode"]=="module" && r["sell"].is_object()){
            for side in ["buy","sell"]{
                let x=&r[side]["exact"];let data=x["transaction"]["data"].as_str().unwrap().parse::<Bytes>().unwrap();
                let call=IVistaFlow::swapFromCall::abi_decode(&data).unwrap();
                let p=Proof{native_router:false,degen_module:true,pool:addr(&r["pool"]),module:addr(&r["module"]),payer:addr(&r["payer"]),input:call.tokenIn,output:call.tokenOut,budget:call.amount,minimum:call.minimum,deadline:call.deadline};
                let logs:Vec<crate::venues::bsc_dex::check::simulate::SimLog>=serde_json::from_value(x["logs"].clone()).unwrap();
                assert_eq!(p.data(),data);assert_eq!(p.measured(&logs),Some((call.amount,call.minimum)));
                let mut missing=logs.clone();missing.retain(|l|l.address!=p.module);
                assert_eq!(p.measured(&missing),None);count+=1;
            }
        }
        assert_eq!(count,92);
    }
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
                    native_router: false,
                    degen_module: false,
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
                let mut remeasured = DirectCall::SafeSwap(std::sync::Arc::new(proof.clone()));
                let stronger = proof.minimum + U256::ONE;
                remeasured.set_remeasured_minimum(stronger).unwrap();
                let encoded = remeasured.data(proof.output, stronger, proof.payer, proof.deadline);
                let updated = IVistaFlow::swapFromCall::abi_decode(&encoded).unwrap();
                assert_eq!(updated.minimum, stronger);

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
