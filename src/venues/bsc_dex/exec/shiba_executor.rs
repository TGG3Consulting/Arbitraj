//! Один платёж Shiba через V5. Исходные остатки исполнителя чужой выручкой не являются.
use super::{chain::Step, executor::call::ExecCall, shiba_payment::Proof, shiba_return};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::venues::bsc_dex::{
    ctx::Ctx,
    node::{abi::IERC20, rpc},
    pools::family_variant::Variant,
    send::track::Receipt,
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, U64, U256, keccak256};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Source {
    pub executor: Address,
    pub hash: B256,
    pub calldata: B256,
}

pub(super) async fn capture(
    ctx: &Ctx,
    legs: &[Vec<Step>],
    call: &ExecCall,
    payer: Address,
) -> GatewayResult<Option<Box<Proof>>> {
    let [leg] = legs else { return Ok(None) };
    let [step] = leg.as_slice() else {
        return Ok(None);
    };
    if step.entry.family.variant != Variant::ShibaswapV2 || call.native.is_some() {
        return Ok(None);
    }
    let bad = || {
        ctx.err(
            ErrorCode::QuoteMismatch,
            "ShibaSwap: форма поручения V5 не соответствует одному проверенному пулу",
        )
    };
    let [route] = call.steps.as_slice() else {
        return Err(bad());
    };
    let pool = step.entry.pool.reference.address();
    let module = ctx.dex_addr("dex.router", step.entry.family.id)?;
    if route.kind != 17
        || route.pool != module
        || route.shareBps != 10000
        || &route.data[..] != pool.into_word().as_slice()
        || (route.tokenIn, route.tokenOut) != (call.token_in, call.token_out)
    {
        return Err(bad());
    }
    let at = crate::venues::bsc_dex::pools::nami::block(ctx, "latest").await?;
    let code: Bytes = rpc::call(ctx, "eth_getCode", json!([call.executor, at])).await?;
    if !matches!(
        super::executor::code::version_of(&code),
        Ok(Some(super::executor::code::Version::V5))
    ) {
        return Err(bad());
    }
    let mut p = Proof {
            chain: crate::venues::bsc_dex::node::chain_id::chain_id(ctx)?,
        module,
        pool,
        payer,
        input: call.token_in,
        output: call.token_out,
        amount: call.amount_in,
        minimum: call.min_out,
        deadline: call.deadline,
        module_hash: B256::ZERO,
        exact_codes: None,
        verified: None,
        v5: Some(Source {
            executor: call.executor,
            calldata: keccak256(call.data()),
            hash: keccak256(code),
        }),
    };
    Proof::bind(ctx, step.entry.family, &mut p).await?;
    Ok(Some(Box::new(p)))
}

// Geth StateAtTransaction возвращает состояние перед выбранной транзакцией. Для последней берём конец её блока.
// https://github.com/ethereum/go-ethereum/blob/master/eth/tracers/api.go
async fn balance(
    ctx: &Ctx,
    at: &Value,
    index: Option<u64>,
    token: Address,
    owner: Address,
    payer: Address,
) -> Option<U256> {
    let data = Bytes::from(IERC20::balanceOfCall { owner }.abi_encode());
    let call = json!({"from":payer,"to":token,"data":data});
    if let Some(i) = index {
        let result=rpc::call::<Value>(ctx,"debug_traceCall",json!([call,at,{"txIndex":format!("{i:#x}"),"tracer":"callTracer","tracerConfig":{"onlyTopCall":true}}])).await.ok()?;
        shiba_return::balance(&result, ctx.chain_id()?, payer, token, data)
    } else {
        let raw = rpc::call::<Bytes>(ctx, "eth_call", json!([call, at]))
            .await
            .ok()?;
        (raw.len() == 32).then(|| U256::from_be_slice(&raw))
    }
}
pub(super) async fn independent(ctx: &Ctx, p: &Proof, r: &Receipt, root: U256) -> Option<()> {
    let e = p.v5.as_ref()?.executor;
    let h = r.block_hash?;
    let at = json!({"blockHash":h,"requireCanonical":true});
    let n = r.transaction_index?.to::<u64>();
    let count: U64 = rpc::call(ctx, "eth_getBlockTransactionCountByHash", json!([h]))
        .await
        .ok()?;
    let last = count.to::<u64>().checked_sub(1)?;
    if n > last {
        return None;
    }
    let next = (n < last).then_some(n + 1);
    let before = Some(n);
    let (wi, after_i, wo, after_o, ei, eo) = tokio::join!(
        balance(ctx, &at, before, p.input, p.payer, p.payer),
        balance(ctx, &at, next, p.input, p.payer, p.payer),
        balance(ctx, &at, before, p.output, p.payer, p.payer),
        balance(ctx, &at, next, p.output, p.payer, p.payer),
        balance(ctx, &at, before, p.input, e, p.payer),
        balance(ctx, &at, before, p.output, e, p.payer)
    );
    own_changes([wi?, after_i?, wo?, after_o?, ei?, eo?], p.amount, root)
}

fn own_changes(
    [before, after, start, end, inventory_in, inventory_out]: [U256; 6],
    amount: U256,
    net: U256,
) -> Option<()> {
    (before.checked_sub(after)? == amount
        && end.checked_sub(start)? == net
        && inventory_in.is_zero()
        && inventory_out.is_zero())
    .then_some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_executor_inventory_and_unlogged_changes_are_not_user_proceeds() {
        let units = |v: u64| U256::from(v);
        let good = [100, 80, 7, 18, 0, 0].map(units);
        assert!(own_changes(good, units(20), units(11)).is_some());
        for (side, replacement) in [(1, 79), (3, 19), (4, 3), (5, 1)] {
            let mut changed = good;
            changed[side] = units(replacement);
            assert!(own_changes(changed, units(20), units(11)).is_none());
        }
    }
}
