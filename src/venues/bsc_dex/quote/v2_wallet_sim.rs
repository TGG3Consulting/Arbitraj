//! Финальное сравнение на фактических балансах кошелька: разрешения, обмен,
//! чистый приход, газ каждого вызова. Оба исполнения используют один родительский блок.
use super::super::{
    check::{
        sim_base::Base,
        simulate::{self, Call, CallResult},
    },
    ctx::Ctx,
    exec::{
        executor::{abi::ArbExecutor, call::ExecCall},
        fork_swap,
    },
    node::rpc,
    wallet::approve,
};
use super::white_state::Head;
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::sol;
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, U256};
use serde_json::json;

sol! {interface IMoney {
    function balanceOf(address owner) external view returns(uint256);
    function allowance(address owner,address spender) external view returns(uint256);
}}
#[derive(Clone)]
pub(super) struct V5Step {
    pub kind: u8,
    pub pool: Address,
    pub data: Bytes,
}
#[derive(Clone)]
pub(super) enum Wire {
    Router02,
    Oasis { rebate: bool },
    V5(V5Step),
}

#[derive(Clone)]
pub(super) struct Order {
    pub payer: Address,
    pub target: Address,
    pub input: Address,
    pub output: Address,
    pub amount: U256,
    pub minimum: U256,
    pub wire: Wire,
}

pub(super) struct Fill {
    pub received: U256,
    pub swap_gas: u64,
    pub approval: (u64, usize),
}
pub(super) async fn measure(ctx: &Ctx, h: Head, o: Order, boundary: bool) -> GatewayResult<Fill> {
    let fail = || {
        ctx.err(
            ErrorCode::QuoteMismatch,
            "V2: средства, газ или поступление обмена не подтверждены",
        )
    };
    let tx = |to, input| Call {
        from: o.payer,
        to,
        input,
    };
    let balance = |token| {
        tx(
            token,
            Bytes::from(IMoney::balanceOfCall { owner: o.payer }.abi_encode()),
        )
    };
    let encoded = Bytes::from(
        IMoney::allowanceCall {
            owner: o.payer,
            spender: o.target,
        }
        .abi_encode(),
    );
    let current: Bytes = rpc::call(
        ctx,
        "eth_call",
        json!([{"to":o.input,"data":encoded},h.selector()]),
    )
    .await?;
    if current.len() != 32 {
        return Err(fail());
    }
    let approvals = approve::fork_known(
        ctx,
        o.input,
        o.target,
        U256::from_be_slice(&current),
        o.amount,
    )?;
    let mut calls = vec![balance(o.input), balance(o.output)];
    calls.extend(approvals.iter().map(|(a, d)| tx(*a, d.clone())));
    let deadline = U256::from(
        h.timestamp
            .to::<u64>()
            .saturating_add(ctx.int("dex.deadline_s")?),
    );
    let data = calldata(&o, deadline, o.minimum);
    calls.extend([tx(o.target, data), balance(o.input), balance(o.output)]);
    let r = frame(ctx, h, &calls).await?;
    if r.iter().any(|x| !x.ok()) {
        return Err(fail());
    }
    let n = approvals.len();
    let word =
        |v: &CallResult| (v.return_data.len() == 32).then(|| U256::from_be_slice(&v.return_data));
    let spent = word(&r[0]).and_then(|old| old.checked_sub(word(&r[n + 3])?));
    let received = word(&r[n + 4])
        .and_then(|after| after.checked_sub(word(&r[1])?))
        .ok_or_else(fail)?;
    if spent != Some(o.amount) || received.is_zero() || received < o.minimum {
        return Err(fail());
    }
    if boundary {
        // Тот же плательщик, исходные остатки и разрешения. Более строгий лишь минимум.
        let strict = received.checked_add(U256::ONE).ok_or_else(fail)?;
        calls[n + 2].input = calldata(&o, deadline, strict);
        let alt = frame(ctx, h, &calls).await?;
        if alt[n + 2].ok()
            || alt[n + 2].error.is_none()
            || alt.iter().enumerate().any(|(j, v)| j != n + 2 && !v.ok())
            || word(&alt[0]) != word(&r[0])
            || word(&alt[1]) != word(&r[1])
            || word(&alt[n + 3]) != word(&r[0])
            || word(&alt[n + 4]) != word(&r[1])
        {
            return Err(fail());
        }
    }
    let approve_gas = r[2..2 + n].iter().map(|x| x.gas_used.to::<u64>()).sum();
    Ok(Fill {
        received,
        swap_gas: r[n + 2].gas_used.to(),
        approval: (approve_gas, n),
    })
}

fn calldata(o: &Order, deadline: U256, minimum: U256) -> Bytes {
    match &o.wire {
        Wire::V5(v5) => {
            let step = ArbExecutor::Step {
                kind: v5.kind,
                pool: v5.pool,
                tokenIn: o.input,
                tokenOut: o.output,
                shareBps: 10000,
                data: v5.data.clone(),
            };
            ExecCall {
                executor: o.target,
                token_in: o.input,
                amount_in: o.amount,
                token_out: o.output,
                min_out: minimum,
                deadline,
                steps: vec![step],
                native: None,
                gated: vec![],
            }
            .data()
        }
        Wire::Oasis { rebate } => super::super::exec::oasis_direct::swap_data(
            o.input, o.output, o.amount, minimum, o.payer, deadline, *rebate,
        ),
        Wire::Router02 => {
            fork_swap::swap_data(o.amount, minimum, &[o.input, o.output], o.payer, deadline)
        }
    }
}

async fn frame(ctx: &Ctx, h: Head, calls: &[Call]) -> GatewayResult<Vec<CallResult>> {
    let mut req = simulate::params(
        calls,
        &[],
        &[],
        Base::timed(h.number.to(), h.timestamp.to()),
    );
    req[1] = h.selector();
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Frame {
        parent_hash: B256,
        calls: Vec<CallResult>,
    }
    let frames: Vec<Frame> = rpc::call(ctx, "eth_simulateV1", req).await?;
    let [frame]: [Frame; 1] = frames
        .try_into()
        .map_err(|_| ctx.err(ErrorCode::BadResponse, "Изменилось число блоков симуляции"))?;
    if frame.parent_hash != h.hash || frame.calls.len() != calls.len() {
        return Err(ctx.err(
            ErrorCode::StaleData,
            "Изменился блок или число откликов симуляции",
        ));
    }
    Ok(frame.calls)
}
