//! Final, fully funded YieldBasis exact-input quotes. Interpolation is only an
//! initial amount for simulation. A revert never establishes monotonicity.
use super::{
    Quoting,
    routing::Input,
    v2_execution::Execution,
    v2_wallet_sim::{self, Fill, Order},
    white_state::Head,
};
use crate::{
    core::types::{Side, yield_basis::Preview},
    gateway::{
        errors::{ErrorCode, GatewayResult},
        quote_stage,
    },
    venues::bsc_dex::{
        math::full_math::mul_div_up,
        node::rpc,
        num,
        pools::{dagrow, registry::Entry, yield_basis, yield_basis_quotes},
    },
};
use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;
use serde_json::json;

fn next(amount: U256, paid: U256, want: U256, cap: U256) -> Option<U256> {
    let adjusted = mul_div_up(amount, want, paid)?;
    let trial = if paid < want {
        adjusted.max(amount.checked_add(U256::ONE)?).min(cap)
    } else {
        adjusted.min(amount.checked_sub(U256::ONE)?)
    };
    (!trial.is_zero() && trial != amount && trial <= cap).then_some(trial)
}
#[allow(clippy::too_many_arguments)]
pub(super) async fn paid(
    q: &Quoting,
    i: &Input,
    e: &Entry,
    s: &Preview,
    payer: Address,
    execution: &Execution,
    h: Head,
) -> GatewayResult<Fill> {
    let ctx = &q.ctx;
    let fail = || {
        ctx.err(
            ErrorCode::NoRoute,
            "YieldBasis: бюджет или фактическая выплата не покрывают объём",
        )
    };
    let buy = i.side == Side::Buy;
    let (input, output) = if buy {
        (i.usdt, i.coin)
    } else {
        (i.coin, i.usdt)
    };
    let want = if buy {
        super::direct::ceil_units(i.qty, i.coin_dec)
    } else {
        num::to_units(i.qty, i.coin_dec)
    }
    .filter(|n| !n.is_zero())
    .ok_or_else(fail)?;
    let measure = |amount, boundary| {
        v2_wallet_sim::measure(
            ctx,
            h,
            Order {
                payer,
                target: execution.target,
                input,
                output,
                amount,
                minimum: U256::ONE,
                wire: execution.wire.clone(),
            },
            boundary,
        )
    };
    if !buy {
        return measure(want, true).await;
    }
    let at = h.selector();
    let native = match &execution.wire {
        v2_wallet_sim::Wire::YieldBasis(t) => !t.value(input, U256::ONE).is_zero(),
        v2_wallet_sim::Wire::V5(v)
        | v2_wallet_sim::Wire::V5Funded(v, _)
        | v2_wallet_sim::Wire::V5Burn(v, _) => {
            matches!(v.native,Some((wrapped,true)) if wrapped==input)
        }
        _ => false,
    };
    let available = if native {
        rpc::call(ctx, "eth_getBalance", json!([payer, at])).await?
    } else {
        dagrow::balance(ctx, input, payer, &at).await?
    };
    let user_cap = quote_stage::buy_budget(ctx.place.id, input)
        .map(|budget| num::to_units(budget, i.usdt_dec).ok_or_else(fail))
        .transpose()?;
    let cap = user_cap.unwrap_or(available).min(available);
    if cap.is_zero() {
        return Err(fail());
    }
    let points = s.points(input).ok_or_else(fail)?;
    // A larger request can use the last sample as an initial slope, but no
    // output beyond a measured amount enters a firm quote.
    let seed = points
        .iter()
        .find(|p| p.amount_out >= want)
        .or(points.last())
        .and_then(|p| mul_div_up(p.amount_in, want, p.amount_out));
    let seed = match seed {
        Some(v) => v.min(cap),
        None => {
            let unit = num::to_units(Decimal::ONE, i.usdt_dec)
                .ok_or_else(fail)?
                .min(cap);
            let nominal = yield_basis_quotes::official(ctx, e, input, unit, &at).await?;
            mul_div_up(unit, want, nominal).ok_or_else(fail)?.min(cap)
        }
    };
    let mut amount = seed.max(U256::ONE);
    // Without client currency-scoped principal, a wallet balance cannot be
    // treated as permission to raise the source amount. Limit any correction
    // to the quote's configured slippage on the initial source estimate.
    let cap = if user_cap.is_some() {
        cap
    } else {
        let raw = num::to_decimal(amount, i.usdt_dec).ok_or_else(fail)?;
        let slip = ctx.dec("exec.dex_slippage_pct")?;
        let budget = raw
            .checked_mul(Decimal::ONE + slip / Decimal::ONE_HUNDRED)
            .ok_or_else(fail)?;
        num::to_units(budget, i.usdt_dec).ok_or_else(fail)?.min(cap)
    };
    let mut best = None;
    // Bound RPC work independently of decimals. Keep the cheapest sufficient
    // sampled debit even if a smaller candidate fails a debt/policy limit.
    let mut attempted = [U256::ZERO; 4];
    for n in 0..attempted.len() {
        if amount.is_zero() || amount > cap || attempted[..n].contains(&amount) {
            break;
        }
        attempted[n] = amount;
        let fill = match measure(amount, false).await {
            Ok(f) => f,
            Err(e)
                if best.is_some()
                    && matches!(e.code, ErrorCode::SwapReverted | ErrorCode::QuoteMismatch) =>
            {
                break;
            }
            Err(e) => return Err(e),
        };
        let got = fill.received;
        if fill.charged != amount {
            return Err(yield_basis::bad(ctx));
        }
        if got >= want
            && best
                .as_ref()
                .is_none_or(|b: &Fill| fill.charged < b.charged)
        {
            best = Some(fill);
        }
        if got == want {
            break;
        }
        let Some(adjusted) = next(amount, got, want, cap) else {
            break;
        };
        amount = adjusted;
    }
    best.ok_or_else(fail)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn funded_amount_correction_is_capped_and_rounds_towards_sufficient_input() {
        let u = U256::from;
        assert_eq!(
            next(u(100u64), u(98u64), u(100u64), u(103u64)),
            Some(u(103u64))
        );
        assert_eq!(next(u(100u64), u(98u64), u(100u64), u(100u64)), None);
        assert_eq!(
            next(u(200u64), u(202u64), u(100u64), u(201u64)),
            Some(u(100u64))
        );
    }
}
