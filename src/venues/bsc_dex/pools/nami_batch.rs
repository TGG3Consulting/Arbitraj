//! Читает ABI и runtime Nami одной пачкой JSON-RPC на каждую известную группу адресов.
use super::super::{
    ctx::Ctx,
    node::{call, rpc},
};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes};
use serde_json::{Value, json};

pub(super) type Request = (&'static str, Value);
pub(super) fn view<C: SolCall>(tag: &Value, to: Address, c: C) -> Request {
    (
        "eth_call",
        json!([{"to":to,"data":Bytes::from(c.abi_encode())},tag]),
    )
}

pub(super) async fn read<const N: usize>(
    ctx: &Ctx,
    req: [Request; N],
) -> GatewayResult<[Bytes; N]> {
    let result = rpc::batch(ctx, &req).await?;
    let mut out = Vec::with_capacity(N);
    for item in result {
        let raw = item?;
        out.push(serde_json::from_str(raw.get()).map_err(|e| {
            ctx.err(
                ErrorCode::BadResponse,
                format!("Элемент пачки RPC не является байтами: {e}"),
            )
        })?);
    }
    out.try_into()
        .map_err(|_| ctx.err(ErrorCode::BadResponse, "Неполная пачка RPC"))
}

pub(super) fn decode<C: SolCall>(raw: &Bytes, _call: C) -> GatewayResult<C::Return> {
    call::decode::<C>(raw)
}
