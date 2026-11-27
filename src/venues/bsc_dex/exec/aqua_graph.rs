//! Фактическая оплата единственной программы через ранее закреплённый исполнитель V5.
use super::{
    aqua_plan,
    aqua_wallet::Proof,
    chain::Step,
    executor::{abi::ArbExecutor, call::ExecCall},
};
use crate::{
    gateway::errors::GatewayResult,
    venues::bsc_dex::{ctx::Ctx, pools::aqua},
};
use alloy::sol_types::{SolCall, SolValue};
use alloy_primitives::{Address, Bytes, keccak256};
pub(super) fn data(p: &Proof) -> Bytes {
    let key = keccak256((p.terms.hash, p.terms.first).abi_encode_params());
    let step = ArbExecutor::Step {
        kind: 17,
        pool: p.terms.target,
        tokenIn: p.input,
        tokenOut: p.output,
        shareBps: 10_000,
        data: Bytes::copy_from_slice(key.as_slice()),
    };
    ArbExecutor::executeCall {
        tokenIn: p.input,
        amountIn: p.amount,
        tokenOut: p.output,
        minOut: p.minimum,
        deadline: p.deadline,
        steps: vec![step],
    }
    .abi_encode()
    .into()
}

pub(super) fn capture(
    ctx: &Ctx,
    legs: &[Vec<Step>],
    call: &ExecCall,
    payer: Address,
) -> GatewayResult<Option<Box<Proof>>> {
    if !legs
        .iter()
        .flatten()
        .any(|step| step.entry.family.variant.aqua())
    {
        return Ok(None);
    }
    let bad = || aqua::bad(ctx);
    let [leg] = legs else { return Err(bad()) };
    let [step] = leg.as_slice() else {
        return Err(bad());
    };
    let [wire] = call.steps.as_slice() else {
        return Err(bad());
    };
    let (id, target) = aqua_plan::graph(ctx, &step.entry).ok_or_else(bad)?;
    if call.native.is_some()
        || wire.kind != 17
        || wire.pool != target
        || wire.shareBps != 10000
        || wire.data.as_ref() != id.as_slice()
        || (call.token_in, call.token_out) != (step.order.token_in, step.order.token_out)
        || (wire.tokenIn, wire.tokenOut) != (call.token_in, call.token_out)
        || call.amount_in != step.order.amount_in
        || call.executor.is_zero()
        || call.executor == payer
        || call.min_out.is_zero()
    {
        return Err(bad());
    }
    let terms = aqua_plan::terms(ctx, &step.entry, target)?;
    let proof = Proof {
        v5: Some(call.executor),
        terms,
        payer,
        input: call.token_in,
        output: call.token_out,
        amount: call.amount_in,
        minimum: call.min_out,
        deadline: call.deadline,
    };
    if proof.data() != call.data() {
        return Err(bad());
    }
    Ok(Some(Box::new(proof)))
}
