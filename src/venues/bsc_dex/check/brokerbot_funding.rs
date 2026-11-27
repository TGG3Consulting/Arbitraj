//! Самофинансирование пробы: используем имеющиеся CHF либо покупаем недостающие
//! за собственный USDT через зарегистрированные V3. Верхняя сумма — check.probe_usdt.
use super::simulate::Call;
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{
        ctx::Ctx,
        exec::{
            chain::Step,
            executor::{self, call, callback_at, onchain},
            plan::Order,
        },
        pools::{
            dagrow,
            exec_kind::ExecKind,
            families::Proto,
            registry::{Entry, Registry},
        },
        quote::{pool_quote, rate},
        wallet::approve_executor,
    },
};
use alloy_primitives::{Address, B256, U256};
use std::sync::Arc;

pub(crate) struct Funding<'a> {
    pub source: Address,
    pub reg: &'a Registry,
    pub usdt: Address,
    pub budget: U256,
}

fn choose(
    ctx: &Ctx,
    reg: &Registry,
    input: Address,
    out: Address,
    wanted: U256,
) -> Option<(U256, Arc<Entry>)> {
    rate::bridge_pools(reg, input, out)
        .into_iter()
        .filter(|e| {
            e.valid
                && e.family.proto == Proto::UniswapV3
                && e.family.executor_kind == Some(ExecKind::V3)
        })
        .filter_map(|e| {
            let x = pool_quote::swap(ctx, &e, input, false, wanted).ok()?;
            (x.complete && x.amount_out >= wanted && !x.amount_in.is_zero())
                .then_some((x.amount_in, e))
        })
        .min_by_key(|v| v.0)
}

pub(super) async fn calls(
    ctx: &Ctx,
    f: &Funding<'_>,
    order: (Address, Address, U256),
    hash: B256,
    now: u64,
) -> GatewayResult<Vec<Call>> {
    let (wallet, cash, amount) = order;
    let at = serde_json::json!({"blockHash":hash,"requireCanonical":true});
    let held = dagrow::balance(ctx, cash, wallet, &at).await?;
    if held >= amount {
        return Ok(vec![]);
    }
    let bad = || {
        ctx.err(ErrorCode::NoRoute,"Aktionariat: недостаточная собственная валюта и нет подтверждённого пополнения в пределах check.probe_usdt")
    };
    let wanted = amount - held;
    let direct = choose(ctx, f.reg, f.usdt, cash, wanted)
        .map(|(paid, e)| vec![(f.usdt, cash, paid, wanted, e)]);
    let through = rate::wrapped_bridge(ctx, f.usdt, cash).and_then(|w| {
        let (mid, last) = choose(ctx, f.reg, w, cash, wanted)?;
        let (paid, first) = choose(ctx, f.reg, f.usdt, w, mid)?;
        Some(vec![
            (f.usdt, w, paid, mid, first),
            (w, cash, mid, wanted, last),
        ])
    });
    let path = direct
        .into_iter()
        .chain(through)
        .min_by_key(|p| p[0].2)
        .ok_or_else(bad)?;
    let paid = path[0].2;
    if paid > f.budget || dagrow::balance(ctx, f.usdt, wallet, &at).await? < paid {
        return Err(bad());
    }
    let target = executor::address(ctx)?.ok_or_else(bad)?;
    let version = onchain::ensure(ctx, target, wallet).await?;
    let n = path.len();
    let mut steps = Vec::with_capacity(n);
    for (i, (input, output, qty, minimum, e)) in path.into_iter().enumerate() {
        if callback_at::missing_at(ctx, e.family, version).is_some() {
            return Err(bad());
        }
        steps.push(Step {
            entry: (*e).clone(),
            order: Order {
                token_in: input,
                token_out: output,
                amount_in: qty,
                min_out: minimum,
            },
            from_wallet: i == 0,
            to_wallet: i + 1 == n,
        });
    }
    let tx = call::build(ctx, &[steps], target, now)?;
    let mut calls: Vec<Call> = approve_executor::all(ctx, f.usdt, target, paid)?
        .into_iter()
        .map(|(to, input)| Call {
            from: wallet,
            to,
            input,
        })
        .collect();
    calls.push(Call {
        from: wallet,
        to: target,
        input: tx.data(),
    });
    Ok(calls)
}
