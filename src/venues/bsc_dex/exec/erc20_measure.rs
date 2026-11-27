//! Симуляция оплаты из собственного кошелька с измерением обеих валют на одном родительском блоке.
use super::super::{
    ctx::Ctx,
    node::{abi::IERC20, rpc},
    quote::white_state,
};
use super::signed_call::Erc20Call;
use crate::gateway::errors::GatewayResult;
use alloy::{rpc::types::Log, sol_types::SolCall};
use alloy_primitives::{Address, B256, Bytes, U64, U256};
use serde::Deserialize;
use serde_json::json;
pub(in crate::venues::bsc_dex) async fn run(
    ctx: &Ctx,
    head: white_state::Head,
    expiry: u64,
    call: &Erc20Call,
    (input, output): (Address, Address),
    data: &Bytes,
) -> GatewayResult<(U256, U256, u64, Vec<Log>)> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Child {
        parent_hash: B256,
        timestamp: U64,
        calls: Vec<Call>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Call {
        status: U64,
        return_data: Bytes,
        gas_used: U64,
        logs: Vec<Log>,
    }
    let bad = || {
        ctx.err(
            crate::gateway::errors::ErrorCode::NoRoute,
            "собственный кошелёк не подтвердил оплату и чистую выплату",
        )
    };
    let who = call.payer;
    let balance = |to| json!({"from":who,"to":to,"input":Bytes::from(IERC20::balanceOfCall{owner:who}.abi_encode())});
    let a = balance(input);
    let b = balance(output);
    let swap = json!({"from":who,"to":call.module,"input":data});
    let blocks: Vec<Child> = rpc::call(
        ctx,
        "eth_simulateV1",
        json!([{"blockStateCalls":[{"calls":[a,b,swap,a,b]}]},head.selector()]),
    )
    .await?;
    let [c] = blocks.as_slice() else {
        return Err(bad());
    };
    let [a, b, op, d, e] = c.calls.as_slice() else {
        return Err(bad());
    };
    if c.parent_hash != head.hash
        || c.timestamp.to::<u64>() > expiry
        || c.calls.iter().any(|v| v.status != U64::from(1))
    {
        return Err(bad());
    }
    let value = |v: &Call| {
        (v.return_data.len() == 32)
            .then(|| U256::from_be_slice(&v.return_data))
            .ok_or_else(bad)
    };
    let debit = value(a)?.checked_sub(value(d)?).ok_or_else(bad)?;
    let credit = value(e)?.checked_sub(value(b)?).ok_or_else(bad)?;
    Ok((debit, credit, op.gas_used.to(), op.logs.clone()))
}
