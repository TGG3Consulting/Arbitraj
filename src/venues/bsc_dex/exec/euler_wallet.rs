//! Официальный EulerSwapPeriphery и выплата проверенного модуля по исходному поручению.
//! Событие пула V2 содержит вход после комиссии. Расход торгового счёта считают по его ERC20-переводам.
use super::{
    fill,
    stabull_wallet::{Form, Proof},
};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{
        ctx::Ctx,
        pools::{
            direct_module, euler_swap, exact_wallet_tokens, family_variant::Variant,
            registry::Entry, solidly_classic_factory,
        },
    },
};
use alloy::{
    rpc::types::Log,
    sol_types::{SolCall, SolEvent},
};
use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;

#[allow(clippy::too_many_arguments)]
mod abi {
    alloy::sol! {
        interface IPeriphery {
            function swapExactIn(address pool,address input,address output,uint256 amount,address receiver,uint256 minimum,uint256 deadline) external;
            function swapExactOut(address pool,address input,address output,uint256 quantity,address receiver,uint256 maximum,uint256 deadline) external;
        }
        interface IV1 {event Swap(address indexed sender,uint256 amount0In,uint256 amount1In,uint256 amount0Out,uint256 amount1Out,uint112 reserve0,uint112 reserve1,address indexed to);}
        interface IV2 {event Swap(address indexed sender,uint256 amount0In,uint256 amount1In,uint256 amount0Out,uint256 amount1Out,uint256 fee0,uint256 fee1,uint112 reserve0,uint112 reserve1,address indexed to);}
    }
}

pub(super) async fn checked(
    ctx: &Ctx,
    e: &Entry,
    pair: (Address, Address),
    target: Address,
    at: &Value,
) -> GatewayResult<Form> {
    let f = e.family;
    let g = ctx.settings.generation();
    let error = || {
        ctx.err(
            ErrorCode::NoRoute,
            "EulerSwap: пул, оператор или форма оплаты изменились",
        )
    };
    let state = euler_swap::market(ctx, f, e.pool.reference.address(), pair, at).await?;
    if !state.installed {
        return Err(error());
    }
    let v2 = f.variant == Variant::EulerV2;
    let guarded =
        if f.executor_kind.is_some() && ctx.dex_addr("dex.router", f.id).ok() == Some(target) {
            direct_module::verify_factory(ctx, f, target, at).await?;
            true
        } else if ctx.dex_addr("dex.quoter", f.id)? == target {
            exact_wallet_tokens::check(ctx, f.id, pair.0, pair.1, at).await?;
            let hash = ctx
                .dex_text("dex.quoter_runtime_hash", f.id)?
                .parse()
                .map_err(|_| error())?;
            solidly_classic_factory::plain(ctx, target, hash, at).await?;
            false
        } else {
            return Err(error());
        };
    if g != ctx.settings.generation() {
        return Err(error());
    }
    Ok(Form::Euler {
        v2,
        guarded,
        vaults: state.payout_vaults[usize::from(pair.1 > pair.0)],
    })
}

pub(super) fn calldata(p: &Proof) -> Bytes {
    let pool = p.terms.pool;
    let input = p.input;
    let output = p.output;
    let receiver = p.payer;
    let deadline = p.deadline;
    match p.terms.desired {
        Some(q) => abi::IPeriphery::swapExactOutCall {
            pool,
            input,
            output,
            quantity: q.max(p.minimum),
            receiver,
            maximum: p.maximum,
            deadline,
        }
        .abi_encode(),
        None => abi::IPeriphery::swapExactInCall {
            pool,
            input,
            output,
            amount: p.maximum,
            receiver,
            minimum: p.minimum,
            deadline,
        }
        .abi_encode(),
    }
    .into()
}

pub(super) fn payment(
    p: &Proof,
    logs: &[Log],
    v2: bool,
    guarded: bool,
    vaults: [Address; 2],
    spent: U256,
    received: U256,
) -> Option<()> {
    let t = p.terms;
    let hash = if v2 {
        abi::IV2::Swap::SIGNATURE_HASH
    } else {
        abi::IV1::Swap::SIGNATURE_HASH
    };
    let mut events = logs
        .iter()
        .filter(|log| log.address() == t.pool && log.topic0() == Some(&hash));
    let log = &events.next()?.inner.data;
    if events.next().is_some() {
        return None;
    }
    let (sender, to, amount0, amount1) = if v2 {
        let ev = abi::IV2::Swap::decode_log_data(log).ok()?;
        (ev.sender, ev.to, ev.amount0Out, ev.amount1Out)
    } else {
        let ev = abi::IV1::Swap::decode_log_data(log).ok()?;
        (ev.sender, ev.to, ev.amount0Out, ev.amount1Out)
    };
    let (nominal, unrelated) = if p.input < p.output {
        (amount1, amount0)
    } else {
        (amount0, amount1)
    };
    if sender != t.target || to != p.payer || !unrelated.is_zero() || nominal < received {
        return None;
    }
    let credited = |vault| fill::moved_logs(logs, p.output, Some(vault), Some(p.payer));
    let direct = credited(vaults[0]);
    let backing = if vaults[0] == vaults[1] {
        direct
    } else {
        direct.checked_add(credited(vaults[1]))?
    };
    if backing != received {
        return None;
    }
    if !guarded
        && (nominal != received
            || fill::moved_logs(logs, p.input, Some(p.payer), Some(t.pool)) != spent)
    {
        return None;
    }
    Some(())
}
