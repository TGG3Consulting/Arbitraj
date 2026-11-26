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
    function getEthBalance(address owner) external view returns(uint256);
    function balanceOf(address owner) external view returns(uint256);
    function allowance(address owner,address spender) external view returns(uint256);
}}
#[derive(Clone)]
pub(in crate::venues::bsc_dex) struct V5Step {
    pub native: Option<(Address, bool)>,
    pub kind: u8,
    pub pool: Address,
    pub data: Bytes,
}
#[derive(Clone)]
pub(in crate::venues::bsc_dex) enum Wire {
    Router02,
    MintClub(super::super::exec::mint_club_wire::Call),
    EkuboV2(super::super::pools::ekubo_v2::OrderKey,Option<Address>),
    DirectModule { pool: Address },
    Vault(B256),
    Vista(super::super::exec::vista_wire::Call),
    Oasis { rebate: bool },
    V5(V5Step),
}

#[derive(Clone)]
pub(in crate::venues::bsc_dex) struct Order {
    pub payer: Address,
    pub target: Address,
    pub input: Address,
    pub output: Address,
    pub amount: U256,
    pub minimum: U256,
    pub wire: Wire,
}

pub(in crate::venues::bsc_dex) struct Fill {
    pub received: U256,
    pub swap_gas: u64,
    pub approval: (u64, usize),
}
pub(in crate::venues::bsc_dex) async fn measure(ctx: &Ctx, h: Head, o: Order, boundary: bool) -> GatewayResult<Fill> {
    let fail = || {
        ctx.err(
            ErrorCode::QuoteMismatch,
            "V2: средства, газ или поступление обмена не подтверждены",
        )
    };
    let mut o=o;
    if let Wire::EkuboV2(key,_ )=&mut o.wire {key.amount=i128::try_from(o.amount).ok().filter(|n|*n>0).ok_or_else(fail)?;}
    let tx = |to, input| Call {
        from: o.payer,
        to,
        input,
    };
    let (wrapped, value) = match &o.wire {
        Wire::EkuboV2(key,wrapped)=>(*wrapped,key.value()),
        Wire::Vista(v) => (
            if v.form == super::super::exec::vista_wire::Form::ModuleToken {
                None
            } else {
                Some(v.wrapped)
            },
            v.value(o.amount),
        ),
        Wire::V5(v) => (
            v.native.map(|(w, _)| w),
            if matches!(v.native, Some((_, true))) {
                o.amount
            } else {
                U256::ZERO
            },
        ),
        _ => (None, U256::ZERO),
    };
    let multicall = if wrapped.is_some() {
        Some(ctx.addr("dex.multicall3")?)
    } else {
        None
    };
    let balance = |token| {
        if let Some(mc) = multicall
            && wrapped == Some(token)
        {
            tx(
                mc,
                Bytes::from(IMoney::getEthBalanceCall { owner: o.payer }.abi_encode()),
            )
        } else {
            tx(
                token,
                Bytes::from(IMoney::balanceOfCall { owner: o.payer }.abi_encode()),
            )
        }
    };
    let approvals = if wrapped == Some(o.input) {
        Vec::new()
    } else {
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
        approve::fork_known(
            ctx,
            o.input,
            o.target,
            U256::from_be_slice(&current),
            match &o.wire { Wire::MintClub(c) if c.mint=>c.limit, _=>o.amount },
        )?
    };
    let mut calls = vec![balance(o.input), balance(o.output)];
    calls.extend(approvals.iter().map(|(a, d)| tx(*a, d.clone())));
    let deadline = U256::from(
        h.timestamp
            .to::<u64>()
            .saturating_add(ctx.int("dex.deadline_s")?),
    );
    let data = calldata(&o, deadline, o.minimum);
    calls.extend([tx(o.target, data), balance(o.input), balance(o.output)]);
    // Vault settles nominal ERC-20 inputs. Confirm that its inventory actually received the charged amount.
    let vault = match &o.wire {
        Wire::Vault(_) => Some(o.target),
        Wire::V5(step) if step.kind == 17 => Some(step.pool),
        _ => None,
    };
    if let Some(holder) = vault {
        let read = || {
            tx(
                o.input,
                Bytes::from(IMoney::balanceOfCall { owner: holder }.abi_encode()),
            )
        };
        calls.insert(0, read());
        calls.push(read());
    }
    let offset = usize::from(vault.is_some());
    let values = (!value.is_zero()).then_some((offset + approvals.len() + 2, value));
    let result = frame(ctx, h, &calls, values.as_slice()).await?;
    if result.iter().any(|x| !x.ok()) {
        return Err(fail());
    }
    let n = approvals.len();
    let word =
        |v: &CallResult| (v.return_data.len() == 32).then(|| U256::from_be_slice(&v.return_data));
    let r = &result[offset..result.len() - offset];
    if vault.is_some()
        && result
            .last()
            .and_then(word)
            .and_then(|after| after.checked_sub(word(&result[0])?))
            != Some(o.amount)
    {
        return Err(fail());
    }
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
        calls[offset + n + 2].input = match &o.wire {
            Wire::MintClub(c)=>c.tighten(spent.ok_or_else(fail)?,received).ok_or_else(fail)?.data(o.payer),
            _=>calldata(&o, deadline, strict),
        };
        let strict_result = frame(ctx, h, &calls, values.as_slice()).await?;
        let alt = &strict_result[offset..strict_result.len() - offset];
        if (vault.is_some()
            && (word(&strict_result[0]) != word(&result[0])
                || strict_result.last().and_then(word) != word(&result[0])))
            || alt[n + 2].ok()
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
        Wire::MintClub(c) => c.data(o.payer),
        Wire::EkuboV2(key,_) => key.calldata(minimum,deadline),
        Wire::Vault(id) => super::super::exec::vault_direct::data(
            *id, o.input, o.output, o.amount, minimum, o.payer, deadline,
        ),
        Wire::Vista(v) => v.data(o.input, o.output, o.amount, minimum, o.payer, deadline),
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
                native: v5.native,
                gated: vec![],
            }
            .data()
        }
        Wire::DirectModule { pool } => super::super::pools::solidly_v3::execution_data(
            *pool, o.input, o.output, o.amount, minimum, deadline,
        ),
        Wire::Oasis { rebate } => super::super::exec::oasis_direct::swap_data(
            o.input, o.output, o.amount, minimum, o.payer, deadline, *rebate,
        ),
        Wire::Router02 => {
            fork_swap::swap_data(o.amount, minimum, &[o.input, o.output], o.payer, deadline)
        }
    }
}

async fn frame(
    ctx: &Ctx,
    h: Head,
    calls: &[Call],
    values: &[(usize, U256)],
) -> GatewayResult<Vec<CallResult>> {
    let mut req = simulate::params(
        calls,
        values,
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
