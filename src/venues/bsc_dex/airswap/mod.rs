//! AirSwap RFQ. Obtain signed terms only from selected makers and replay the actual payer's call.
mod book;
mod client;
mod indicative;
mod offers;
mod profile;
mod ranking;
pub(super) mod receipt;
pub(super) mod routing;
mod signed;
#[cfg(test)]
mod tests;
mod trade;
mod wire;
pub(super) use book::Book;
pub(super) use trade::trade;
pub(super) const LABEL: &str = "AirSwap RFQ";
use super::{ctx::Ctx, num, parts::Parts, quote::white_state, wallet};
use crate::gateway::{
    errors::{ErrorCode, GatewayError, GatewayResult},
    source_quote::{Preview, Request},
};
use alloy_primitives::U256;
use client::Query;
use serde_json::json;
fn no(ctx: &Ctx, m: &str) -> GatewayError {
    ctx.err(ErrorCode::NoRoute, m)
}
pub(super) fn enabled(ctx: &Ctx) -> bool {
    ctx.flag("airswap.trading").unwrap_or(false)
}
pub(super) fn timely(ctx: &Ctx, exp: u64) -> bool {
    let now = chrono::Utc::now().timestamp_millis().max(0) as u64;
    exp.checked_mul(1000).is_some_and(|end| {
        end > now
            && ctx
                .int("airswap.min_validity_ms")
                .is_ok_and(|m| end - now >= m)
    })
}
#[derive(Clone, Copy)]
struct Limits {
    minimum: U256,
    maximum: U256,
}
pub(super) async fn request(p: &Parts, r: Request) -> GatewayResult<Preview> {
    let c = &p.ctx;
    let bad = || {
        no(
            c,
            "AirSwap: укажите два ERC20 и один размер в минимальных единицах",
        )
    };
    if r.input == r.output
        || r.input.is_zero()
        || r.output.is_zero()
        || r.market.is_some()
        || r.builder.is_some()
    {
        return Err(bad());
    }
    let exact_out = r.buy_amount_raw.is_some();
    let amount = if exact_out {
        if r.amount_raw.is_some() || r.amount_tokens.is_some() {
            return Err(bad());
        }
        num::exact_amount(r.buy_amount_raw.as_deref(), None, 0)
    } else if r.amount_tokens.is_some() {
        num::exact_amount(
            r.amount_raw.as_deref(),
            r.amount_tokens.as_deref(),
            p.tokens.decimals(c, r.input).await?,
        )
    } else {
        num::exact_amount(r.amount_raw.as_deref(), None, 0)
    }
    .ok_or_else(bad)?;
    let q = Query {
        input: r.input,
        output: r.output,
        amount,
        exact_out,
    };
    let mut report = Preview {
        source: "airswap".into(),
        input: q.input,
        output: q.output,
        spent_raw: String::new(),
        received_raw: String::new(),
        unspent_raw: "0".into(),
        fee: None,
        block_hash: String::new(),
        block: None,
        expires: None,
        details: json!({"type":"indicative"}),
    };
    let (spent, received) = if let Some(f) = r.firm {
        let account = p.bound(&f.wallet);
        let c = &account.ctx;
        c.bound()?;
        let owner = wallet::signer::address(c)?;
        if r.trader != Some(owner) || f.fee_tenths_bps.is_some() || (f.allow_trade && !enabled(c)) {
            return Err(bad());
        }
        let minimum = num::exact_amount(Some(&f.minimum_raw), None, 0).ok_or_else(bad)?;
        let maximum = match (exact_out, f.maximum_raw.as_deref()) {
            (true, Some(raw)) => num::exact_amount(Some(raw), None, 0).ok_or_else(bad)?,
            (false, None) => amount,
            _ => return Err(bad()),
        };
        let limits = Limits {
            minimum: if exact_out {
                minimum.max(amount)
            } else {
                minimum
            },
            maximum,
        };
        let head = white_state::head(c, "latest").await?;
        let tickets = offers::options(&account, q, limits, owner, head).await?;
        // Report the best measured cash amounts; route selection also prices the gas of every form.
        let best = tickets
            .iter()
            .min_by(|a, b| {
                if exact_out {
                    a.spent.cmp(&b.spent).then(a.gas.cmp(&b.gas))
                } else {
                    b.received.cmp(&a.received).then(a.gas.cmp(&b.gas))
                }
            })
            .ok_or_else(bad)?;
        let values = (best.spent, best.received);
        report.unspent_raw = maximum.checked_sub(best.spent).ok_or_else(bad)?.to_string();
        report.block = Some(head.number.to());
        report.block_hash = format!("{:#x}", head.hash);
        report.expires = Some(best.proof.expiry);
        report.details = json!({"type":"wallet_simulation","candidates":tickets.len(),"allow_trade":f.allow_trade,"ticket":best.proof.call.calldata,"maker":best.proof.maker,"gas_used":best.gas,"signed_digest":best.proof.digest,"owner_minimum_raw":best.proof.minimum.to_string()});
        if f.allow_trade {
            for t in tickets {
                p.airswap.put(t);
            }
        }
        values
    } else {
        indicative::level(p, q).await?
    };
    report.spent_raw = spent.to_string();
    report.received_raw = received.to_string();
    Ok(report)
}
