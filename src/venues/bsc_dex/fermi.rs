//! Котировка FermiSwap в планируемом Ethereum-блоке выбранного строителя.
//! https://docs.titanbuilder.xyz/propamms · https://buildernet.org/docs/api
//! Сверяет chain/router/engine, родителя, номер и время слота до и после запроса.
//! Операторская котировка приходит из общего шлюза. Штатный выбор и порядок отправки реализованы в дочерних модулях.
pub(super) mod bopamm;
mod deployment;
mod frame;
pub(super) mod markets;
pub(super) mod routing;
pub(super) use execute::trade;
pub(super) use recovery::reconcile;
mod dispatch;
mod execute;
mod indicative;
mod levels;
mod missed;
mod receipt;
mod recovery;
mod slot;
#[cfg(test)]
mod tests;
use super::{ctx::Ctx, node::rpc};
use crate::gateway::{
    errors::{ErrorCode, GatewayResult},
    http,
    source_quote::{Preview, Request},
};
use alloy::{
    sol,
    sol_types::{SolCall, SolValue},
};
use alloy_primitives::{Address, B256, Bytes, I256, U64, U256, keccak256};
use chrono::Utc;
use deployment::Deployment;
use frame::{Builder, Frame};
use serde::Deserialize;
use serde_json::{Value, json};

sol! {
    function quoteAmounts(address tokenIn, address tokenOut, int256 amountSpecified) external view
        returns (uint256 amountIn, uint256 amountOut);
    function fermi() external view returns (address);
    function decimals() external view returns (uint8);
    struct Pair {address asset0; address asset1;}
    function getPairs() external view returns (Pair[]);
}
#[derive(Clone, Deserialize)]
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
    gas_used: U64,
    status: U64,
    return_data: Bytes,
}

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

async fn verify_router(ctx: &Ctx, head: &Head, deployment: Deployment) -> GatewayResult<()> {
    let tag = json!({"blockHash":head.hash,"requireCanonical":true});
    let (code, engine): (Bytes, Address) = tokio::try_join!(
        rpc::call(ctx, "eth_getCode", json!([deployment.router, tag])),
        canonical(ctx, head.hash, deployment.router, fermiCall {})
    )?;
    if keccak256(code) != deployment.runtime || engine != deployment.engine {
        return Err(ctx.err(
            ErrorCode::NotApplicable,
            "Fermi: изменился код роутера или оракул",
        ));
    }
    Ok(())
}

async fn builder_snapshot(ctx: &Ctx, builder: Builder) -> GatewayResult<Value> {
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
            "Не получено состояние строителя Ethereum",
        )
    })?;
    let parsed = serde_json::from_str::<Value>(&body).ok();
    parsed
        .filter(|r| {
            status.is_success() && r["id"] == 1 && r["jsonrpc"] == "2.0" && r.get("error").is_none()
        })
        .and_then(|mut r| r.get_mut("result").map(Value::take))
        .ok_or_else(|| {
            ctx.err(
                ErrorCode::BadResponse,
                "Некорректный ответ строителя Ethereum",
            )
        })
}

async fn frame(ctx: &Ctx, builder: Builder, deployment: Deployment) -> GatewayResult<Frame> {
    Frame::read(builder, &builder_snapshot(ctx, builder).await?, deployment).ok_or_else(|| {
        ctx.err(
            ErrorCode::BadResponse,
            "В кадре строителя нет оракула Fermi",
        )
    })
}

fn exact_amount(raw: Option<&str>, tokens: Option<&str>, precision: u8) -> Option<U256> {
    super::num::exact_amount(raw, tokens, precision).filter(|v| v.bit_len() <= 255)
}

pub(super) async fn source_preview(ctx: &Ctx, req: Request) -> GatewayResult<Preview> {
    let deployment = Deployment::read(ctx)?;
    let revision = ctx.settings.generation();
    let bad = |message| ctx.err(ErrorCode::NotApplicable, message);
    if ctx.chain_id() != Some(1)
        || req.input == req.output
        || req.input.is_zero()
        || req.output.is_zero()
        || req
            .market
            .is_some_and(|m| m != deployment.router && Some(m) != ctx.addr("fermi.executor").ok())
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
        frame(ctx, builder, deployment)
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
    let (_, precision) = tokio::try_join!(
        verify_router(ctx, &head, deployment),
        canonical(ctx, head.hash, req.input, decimalsCall {})
    )?;
    let amount = exact_amount(
        req.amount_raw.as_deref(),
        req.amount_tokens.as_deref(),
        precision,
    )
    .ok_or_else(|| {
        bad("Fermi: нужна одна положительная сумма, точно представимая в единицах токена и int256")
    })?;
    let executor = req.market.filter(|m| *m != deployment.router);
    if let Some(address) = executor {
        slot::verify_executor(
            ctx,
            &head,
            address,
            deployment,
            deployment::runtime(ctx, "fermi.executor_runtime_hash")?,
        )
        .await?;
    }
    let signed = I256::from_raw(amount);
    let (_, quoted) = slot::amounts(
        ctx,
        &head,
        &frame,
        executor.unwrap_or(who),
        req.input,
        req.output,
        signed,
    )
    .await?;
    let (received, gas_used) = if let Some(executor) = executor {
        let leg = keccak256((head.hash, who, req.input, req.output, amount).abi_encode());
        let proof = slot::Proof {
            builder: builder.name().into(),
            parent: head.hash,
            target: frame.target,
            timestamp: frame.timestamp,
            executor,
            router: Some(deployment.router),
            payer: who,
            input: req.input,
            output: req.output,
            amount: signed,
            limit: quoted,
            leg,
        };
        let (_, received, gas) = slot::simulate(ctx, &head, &frame, &proof, None).await?;
        (received, Some(gas))
    } else {
        (quoted, None)
    };
    let latest: Head = rpc::call(ctx, "eth_getBlockByNumber", json!(["latest", false])).await?;
    if latest.hash != head.hash
        || !current()
        || revision != ctx.settings.generation()
        || Deployment::read(ctx)? != deployment
    {
        return Err(stale());
    }
    Ok(Preview {
        source: "fermi".into(),
        input: req.input,
        output: req.output,
        spent_raw: amount.to_string(),
        received_raw: received.to_string(),
        unspent_raw: "0".into(),
        fee: None,
        block_hash: head.hash.to_string(),
        block: Some(head.number.to::<u64>()),
        expires: Some(frame.timestamp),
        details: json!({"router":deployment.router,"engine":deployment.engine,"builder":builder_id,"trader":who,
            "target_block":frame.target,"slot":frame.slot,"quote_only":executor.is_none(),"simulated_executor":executor,"gas_used":gas_used}),
    })
}
