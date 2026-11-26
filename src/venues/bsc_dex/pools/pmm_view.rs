//! Типизированный вызов PMM от действительного кошелька на одном каноническом состоянии.
use super::super::{
    ctx::Ctx,
    node::{call, rpc},
};
use crate::gateway::errors::GatewayResult;
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes};
use serde_json::{Value, json};

pub(super) async fn view<C: SolCall>(
    ctx: &Ctx,
    at: &Value,
    from: Address,
    to: Address,
    input: C,
) -> GatewayResult<C::Return> {
    let bytes = Bytes::from(input.abi_encode());
    let raw: Bytes = rpc::call(
        ctx,
        "eth_call",
        json!([{"from":from,"to":to,"data":bytes},at]),
    )
    .await?;
    call::decode::<C>(&raw)
}
