//! An issuer fill inside a signed V5 graph has its own settlement currency.
//! Only the two endpoint tokens count towards the order's debit and proceeds.
use super::{
    aktionariat_plan, aktionariat_wallet::{IAktionariatWallet, Market, Proof, step_id},
    chain::Step, executor::call::ExecCall, fill, signed_call::Erc20Call,
};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, node::abi::IERC20},
};
use alloy::{rpc::types::Log, sol_types::SolEvent};
use alloy_primitives::{Address, U256, keccak256};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct IssuerStep {
    pub market: Market,
    pub input: Address,
    pub output: Address,
    pub cap: Option<U256>,
}

pub(super) fn capture(ctx: &Ctx, legs: &[Vec<Step>], call: &ExecCall, payer: Address) -> GatewayResult<Option<Box<Proof>>> {
    if call.native.is_some() || !legs.iter().flatten().any(|s| s.entry.family.variant.issuer_shares()) {
        // Native endpoints use the existing native value/unwrap settlement path.
        return Ok(None);
    }
    let bad = || ctx.err(ErrorCode::QuoteMismatch, "Aktionariat: шаги подписываемого маршрута отличаются от котировки");
    let mut encoded = call.steps.iter();
    let mut issuer_steps = Vec::new();
    for leg in legs {
        for (index, step) in leg.iter().enumerate() {
            let wire = encoded.next().ok_or_else(bad)?;
            if step.entry.family.variant.issuer_shares() {
                let market = aktionariat_plan::terms(ctx, &step.entry)?;
                let cap = (index == 0).then_some(step.order.amount_in);
                let id = step_id(market.broker, cap).ok_or_else(bad)?;
                if wire.kind != 17 || wire.pool != market.module || wire.data.as_ref() != id.as_slice() ||
                   (wire.tokenIn, wire.tokenOut) != (step.order.token_in, step.order.token_out) {
                    return Err(bad());
                }
                issuer_steps.push(IssuerStep { market, input: wire.tokenIn, output: wire.tokenOut, cap });
            }
        }
    }
    let market = issuer_steps.first().ok_or_else(bad)?.market;
    if encoded.next().is_some() { return Err(bad()); }
    Ok(Some(Box::new(Proof {
        market, payer, input: call.token_in, output: call.token_out, budget: call.amount_in,
        minimum: call.min_out, deadline: call.deadline, issuer_steps,
        v5: Some(Erc20Call {chain: 1, payer, module: call.executor, calldata: keccak256(call.data())}),
    })))
}

/// A sweep of inventory that the executor held before this swap cannot fund an issuer step
/// or count as a refund or proceeds. V3 callbacks can credit their output before collecting input.
fn own_inventory(logs: &[Log], executor: Address) -> Option<()> {
    let mut acquired = HashMap::<Address,U256>::new();
    for log in logs {
        let Ok(t) = IERC20::Transfer::decode_log_data(&log.inner.data) else {continue;};
        if t.from == t.to || (t.from != executor && t.to != executor) {continue;}
        let balance = acquired.entry(log.address()).or_default();
        *balance = if t.from == executor {balance.checked_sub(t.value)?} else {balance.checked_add(t.value)?};
    }
    Some(())
}

pub(super) fn amounts(proof: &Proof, logs: &[Log]) -> Option<(U256,U256)> {
    let executor = proof.v5?.module;
    own_inventory(logs, executor)?;
    let mut expected = proof.issuer_steps.iter();
    let mut start = 0;
    for (index, log) in logs.iter().enumerate() {
        if log.topic0() != Some(&IAktionariatWallet::Paid::SIGNATURE_HASH) ||
            !proof.issuer_steps.iter().any(|s| s.market.module == log.address()) { continue; }
        let step = expected.next()?;
        let paid = IAktionariatWallet::Paid::decode_log_data(&log.inner.data).ok()?;
        let recipient = if step.output == proof.output { proof.payer } else { executor };
        if log.address() != step.market.module || (paid.payer, paid.recipient, paid.broker, paid.source, paid.target) !=
                (executor, recipient, step.market.broker, step.input, step.output) ||
           paid.spent.is_zero() || paid.received.is_zero() || step.cap.is_some_and(|cap| paid.spent > cap) {
            return None;
        }
        let segment = &logs[start..index];
        let moved = |token,from,to| fill::moved_logs(segment,token,Some(from),Some(to));
        let debit = moved(step.input,executor,step.market.module)
            .checked_sub(moved(step.input,step.market.module,executor))?;
        if debit != paid.spent || moved(step.output,step.market.module,recipient) != paid.received {return None;}
        start = index + 1;
    }
    if expected.next().is_some() {return None;}
    let net = |token,from,to| fill::moved_logs(logs,token,from,to);
    let spent = net(proof.input,Some(proof.payer),None).checked_sub(net(proof.input,None,Some(proof.payer)))?;
    let received = net(proof.output,None,Some(proof.payer)).checked_sub(net(proof.output,Some(proof.payer),None))?;
    (!spent.is_zero() && spent <= proof.budget && received >= proof.minimum).then_some((spent,received))
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::executor::abi::ArbExecutor;
    use alloy::sol_types::SolCall;
    use alloy_primitives::{B256, Bytes};
    use serde_json::Value;

    #[test]
    fn funded_usdt_routes_count_primary_cash_once_and_verify_each_issuer_payment() {
        let archive = include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/aktionariat_ethereum_usdt.json.gz");
        let sample: Value = serde_json::from_reader(flate2::read::GzDecoder::new(archive.as_slice())).unwrap();
        let addr = |v: &Value| v.as_str().unwrap().parse::<Address>().unwrap();
        let units = |v: &Value| v.as_str().unwrap().parse::<U256>().unwrap();
        let module = addr(&sample["module"]);
        let payer = addr(&sample["owner"]);
        let executor = addr(&sample["executor"]);
        let mut covered = 0;
        for row in sample["controls"].as_array().unwrap() {
            let market = Market { broker: addr(&row["broker"]), module, runtime: B256::ZERO };
            for (index, side) in ["buy","sell"].iter().enumerate() {
                let data: Bytes = row[*side]["data"].as_str().unwrap().parse().unwrap();
                let tx = ArbExecutor::executeCall::abi_decode(&data).unwrap();
                let broker = tx.steps.iter().find(|s| s.pool == module && s.kind == 17).unwrap();
                let step = IssuerStep { market, input: broker.tokenIn, output: broker.tokenOut,
                    cap: (index == 1).then_some(tx.amountIn) };
                let proof = Proof { market, payer, input: tx.tokenIn, output: tx.tokenOut, budget: tx.amountIn,
                    minimum: tx.minOut, deadline: tx.deadline, issuer_steps: vec![step],
                    v5: Some(Erc20Call {chain:1,payer,module:executor,calldata:keccak256(data)}) };
                let logs: Vec<Log> = serde_json::from_value(row["logs"][index].clone()).unwrap();
                let expected = if index == 0 { (units(&row["budget"]),units(&row["units"])) }
                               else { (units(&row["units"]),units(&row["returned"])) };
                assert_eq!(amounts(&proof,&logs),Some(expected));
                let mut missing = logs.clone();
                missing.retain(|l| l.address()!=module || l.topic0()!=Some(&IAktionariatWallet::Paid::SIGNATURE_HASH));
                assert!(amounts(&proof,&missing).is_none());
                let mut duplicated = logs.clone();
                let event = logs.iter().find(|l| l.address()==module && l.topic0()==Some(&IAktionariatWallet::Paid::SIGNATURE_HASH)).unwrap();
                duplicated.push(event.clone());
                assert!(amounts(&proof,&duplicated).is_none());
                // With the payer's funding removed, the same swap appears to spend old executor inventory.
                let mut ambient = logs.clone();
                let funding = ambient.iter().position(|l| l.address()==proof.input &&
                    IERC20::Transfer::decode_log_data(&l.inner.data).is_ok_and(|t| t.from==payer && t.to==executor)).unwrap();
                ambient.remove(funding);
                assert!(amounts(&proof,&ambient).is_none());
                let mut wrong = proof.clone();
                wrong.issuer_steps[0].market.broker = payer;
                assert!(amounts(&wrong,&logs).is_none());
                covered += 1;
            }
        }
        assert_eq!(covered,8);
    }
}
