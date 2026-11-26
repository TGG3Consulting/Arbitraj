//! Котировка FermiSwap в планируемом Ethereum-блоке выбранного строителя.
//! https://docs.titanbuilder.xyz/propamms · https://buildernet.org/docs/api
//! Сверяет chain/router/engine, родителя, номер и время слота до и после запроса.
//! Только HTTP-чтение по запросу оператора через общий шлюз источников.
mod frame;
#[cfg(test)]
mod tests;
use super::{ctx::Ctx, node::rpc};
use crate::gateway::{
    errors::{ErrorCode, GatewayResult},
    http,
    source_quote::{Preview, Request},
};
use alloy::{sol, sol_types::SolCall};
use alloy_primitives::{Address, B256, Bytes, I256, U64, U256, b256, keccak256};
use chrono::Utc;
use frame::{Builder, ENGINE, Frame, ROUTER};
use serde::Deserialize;
use serde_json::{Value, json};

sol! {
    function quoteAmounts(address tokenIn, address tokenOut, int256 amountSpecified) external view
        returns (uint256 amountIn, uint256 amountOut);
    function fermi() external view returns (address);
    function decimals() external view returns (uint8);
}
#[derive(Deserialize)]
struct Head {
    number: U64,
    hash: B256,
    timestamp: U64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SimBlock {
    number: U64,
    parent_hash: B256,
    timestamp: U64,
    calls: Vec<SimCall>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SimCall {
    status: U64,
    return_data: Bytes,
}

const RUNTIME: B256 = b256!("7b309d240875f4c0767cb5834a8c580f74de2f0b509068c3e83aa65321f39a54");

async fn canonical<C: SolCall>(
    ctx: &Ctx,
    hash: B256,
    to: Address,
    method: C,
) -> GatewayResult<C::Return> {
    let data = Bytes::from(method.abi_encode());
    let raw: Bytes = rpc::call(
        ctx,
        "eth_call",
        json!([{"to":to,"data":data},{"blockHash":hash,"requireCanonical":true}]),
    )
    .await?;
    C::abi_decode_returns(&raw).map_err(|_| {
        ctx.err(
            ErrorCode::BadResponse,
            "Fermi: неверный ABI на каноническом блоке",
        )
    })
}

async fn frame(ctx: &Ctx, builder: Builder) -> GatewayResult<Frame> {
    let (url, method) = builder.data();
    let mut limits = ctx.limits()?;
    limits.max_bytes = limits.max_bytes.min(1_048_576);
    let request = ctx
        .http
        .post(url)
        .json(&json!({"jsonrpc":"2.0","id":1,"method":method,"params":[]}));
    let (status, body) = http::exchange(request, limits).await.map_err(|_| {
        ctx.err(
            ErrorCode::Unreachable,
            "Fermi: не получено состояние строителя",
        )
    })?;
    let parsed = serde_json::from_str::<Value>(&body).ok();
    parsed
        .as_ref()
        .filter(|r| {
            status.is_success() && r["id"] == 1 && r["jsonrpc"] == "2.0" && r.get("error").is_none()
        })
        .and_then(|r| Frame::read(builder, &r["result"]))
        .ok_or_else(|| {
            ctx.err(
                ErrorCode::BadResponse,
                "Fermi: некорректный ответ строителя или нет состояния оракула",
            )
        })
}

fn exact_amount(raw: Option<&str>, tokens: Option<&str>, precision: u8) -> Option<U256> {
    let digits = match (raw, tokens) {
        (Some(v), None) => v.to_owned(),
        (None, Some(v)) => {
            let (whole, fraction) = v.split_once('.').unwrap_or((v, ""));
            if whole.is_empty()
                || !whole.bytes().all(|b| b.is_ascii_digit())
                || !fraction.bytes().all(|b| b.is_ascii_digit())
            {
                return None;
            }
            let fraction = fraction.trim_end_matches('0');
            let pad = usize::from(precision).checked_sub(fraction.len())?;
            format!("{whole}{fraction}{}", "0".repeat(pad))
        }
        _ => return None,
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    U256::from_str_radix(&digits, 10)
        .ok()
        .filter(|v| !v.is_zero() && v.bit_len() <= 255)
}

pub(super) async fn source_preview(ctx: &Ctx, req: Request) -> GatewayResult<Preview> {
    let bad = |message| ctx.err(ErrorCode::NotApplicable, message);
    if ctx.chain_id() != Some(1)
        || req.input == req.output
        || req.input.is_zero()
        || req.output.is_zero()
        || req.market.is_some_and(|m| m != ROUTER)
    {
        return Err(bad(
            "Fermi: требуется Ethereum, текущий роутер и два разных ERC-20; для ETH используйте WETH",
        ));
    }
    let who = req
        .trader
        .filter(|v| !v.is_zero())
        .ok_or_else(|| bad("Fermi: укажите адрес трейдера"))?;
    let builder_id = req
        .builder
        .as_deref()
        .ok_or_else(|| bad("Fermi: укажите titan или buildernet"))?;
    let builder =
        Builder::parse(builder_id).ok_or_else(|| bad("Fermi: неизвестный строитель Ethereum"))?;
    let (network, head, frame): (U64, Head, Frame) = tokio::try_join!(
        rpc::call(ctx, "eth_chainId", json!([])),
        rpc::call(ctx, "eth_getBlockByNumber", json!(["latest", false])),
        frame(ctx, builder)
    )?;
    if network.to::<u64>() != 1 {
        return Err(bad("Fermi: узел принадлежит другой сети"));
    }
    let now = || Utc::now().timestamp().max(0) as u64;
    let current = || frame.current(head.number.to::<u64>(), head.timestamp.to::<u64>(), now());
    let stale = || {
        ctx.err(
            ErrorCode::StaleData,
            "Fermi: окно слота закрыто либо сменился родитель",
        )
    };
    if !current() {
        return Err(stale());
    }
    let tag = json!({"blockHash":head.hash,"requireCanonical":true});
    let (code, engine, precision): (Bytes, Address, u8) = tokio::try_join!(
        rpc::call(ctx, "eth_getCode", json!([ROUTER, tag])),
        canonical(ctx, head.hash, ROUTER, fermiCall {}),
        canonical(ctx, head.hash, req.input, decimalsCall {})
    )?;
    if keccak256(&code) != RUNTIME || engine != ENGINE {
        return Err(bad("Fermi: изменился код роутера или оракул"));
    }
    let amount = exact_amount(
        req.amount_raw.as_deref(),
        req.amount_tokens.as_deref(),
        precision,
    )
    .ok_or_else(|| {
        bad("Fermi: нужна одна положительная сумма, точно представимая в единицах токена и int256")
    })?;
    let q = quoteAmountsCall {
        tokenIn: req.input,
        tokenOut: req.output,
        amountSpecified: I256::from_raw(amount),
    };
    let body = json!({"blockStateCalls":[{"blockOverrides":{"time":U64::from(frame.timestamp)},
        "stateOverrides":frame.overrides(),"calls":[{"from":who,"to":ROUTER,"data":Bytes::from(q.abi_encode())}]}]});
    let blocks: Vec<SimBlock> = rpc::call(ctx, "eth_simulateV1", json!([body, tag])).await?;
    let [b] = blocks.as_slice() else {
        return Err(bad("Fermi: неверное число блоков симуляции"));
    };
    if b.parent_hash != head.hash
        || b.number.to::<u64>() != frame.target
        || b.timestamp.to::<u64>() != frame.timestamp
    {
        return Err(stale());
    }
    let [c] = b.calls.as_slice() else {
        return Err(bad("Fermi: неверное число вызовов симуляции"));
    };
    let q = quoteAmountsCall::abi_decode_returns(&c.return_data)
        .ok()
        .filter(|q| c.status.to::<u64>() == 1 && q.amountIn == amount && !q.amountOut.is_zero())
        .ok_or_else(|| {
            ctx.err(
                ErrorCode::NoRoute,
                "Fermi: котировка стороны и точного объёма недоступна",
            )
        })?;
    let latest: Head = rpc::call(ctx, "eth_getBlockByNumber", json!(["latest", false])).await?;
    if latest.hash != head.hash || !current() {
        return Err(stale());
    }
    Ok(Preview {
        source: "fermi".into(),
        input: req.input,
        output: req.output,
        spent_raw: amount.to_string(),
        received_raw: q.amountOut.to_string(),
        unspent_raw: "0".into(),
        fee: None,
        block_hash: head.hash.to_string(),
        block: Some(head.number.to::<u64>()),
        expires: Some(frame.timestamp),
        details: json!({"router":ROUTER,"engine":ENGINE,"builder":builder_id,"trader":who,
            "target_block":frame.target,"slot":frame.slot,"quote_only":true}),
    })
}
