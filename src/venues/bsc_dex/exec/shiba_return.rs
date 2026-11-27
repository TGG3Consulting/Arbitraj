//! Читает только корневой результат конкретной добытой транзакции по тому же RPC узлу.
use super::signed_call::Erc20Call;
use crate::venues::bsc_dex::{ctx::Ctx, node::rpc, send::track::Receipt};
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
struct Call {
    from: Address,
    to: Address,
    input: Bytes,
    #[serde(default)]
    value: U256,
    #[serde(default)]
    error: Option<String>,
}
#[derive(Deserialize)]
struct Root {
    #[serde(flatten)]
    call: Call,
    #[serde(rename = "type")]
    kind: String,
    output: Bytes,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Located {
    transaction_hash: B256,
    block_hash: B256,
    transaction_position: u64,
    trace_address: Vec<u64>,
    action: Call,
    result: RootOutput,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    error: Option<String>,
}
#[derive(Deserialize)]
struct RootOutput {
    output: Bytes,
}

fn net(call: &Call, output: &Bytes, order: Erc20Call) -> Option<U256> {
    (call.error.is_none()
        && (call.from, call.to) == (order.payer, order.module)
        && call.value.is_zero()
        && keccak256(&call.input) == order.calldata
        && output.len() == 32)
        .then(|| U256::from_be_slice(output))
}
pub(super) async fn read(ctx: &Ctx, r: &Receipt, hash: B256, order: Erc20Call) -> Option<U256> {
    if let Ok(Some(t)) = rpc::call::<Option<Located>>(ctx, "trace_get", json!([hash, []])).await
        && t.trace_address.is_empty()
        && t.kind == "call"
        && t.error.is_none()
        && t.transaction_hash == hash
        && Some(t.block_hash) == r.block_hash
        && Some(t.transaction_position) == r.transaction_index.map(|v| v.to::<u64>())
        && let Some(n) = net(&t.action, &t.result.output, order)
    {
        return Some(n);
    }
    let t = rpc::call::<Root>(
        ctx,
        "debug_traceTransaction",
        json!([hash,{"tracer":"callTracer","tracerConfig":{"onlyTopCall":true}}]),
    )
    .await
    .ok()?;
    if t.kind != "CALL" {
        return None;
    }
    net(&t.call, &t.output, order)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_the_signed_root_output_is_accepted() {
        let mut c = Call {
            from: Address::repeat_byte(1),
            to: Address::repeat_byte(2),
            input: Bytes::from_static(&[1, 2, 3]),
            value: U256::ZERO,
            error: None,
        };
        let s = Erc20Call {
            payer: c.from,
            module: c.to,
            chain: 1,
            calldata: keccak256(&c.input),
        };
        let out = Bytes::copy_from_slice(&U256::from(123).to_be_bytes::<32>());
        assert_eq!(net(&c, &out, s), Some(U256::from(123)));
        assert_eq!(net(&c, &Bytes::from_static(&[0, 1]), s), None);
        c.error = Some("revert".into());
        assert_eq!(net(&c, &out, s), None);
        c.error = None;
        c.to = c.from;
        assert_eq!(net(&c, &out, s), None);
    }
}
