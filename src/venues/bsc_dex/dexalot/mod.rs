//! Same-chain Dexalot orders. The public reference price may use ETH as the WETH price source.
mod client;
mod firm;
mod price;
mod profile;
pub(super) mod receipt;
pub(super) mod routing;
#[cfg(test)]
mod tests;
mod trade;
mod wire;
pub(super) use routing::Book;
pub(super) use trade::trade;
pub(super) const LABEL: &str = "Dexalot RFQ";
pub(super) fn enabled(ctx: &Ctx) -> bool {
    ctx.flag("dexalot.trading").unwrap_or(false)
}
pub(super) fn timely(ctx: &Ctx, exp: u64) -> bool {
    let now = chrono::Utc::now().timestamp_millis().max(0) as u64;
    exp.checked_mul(1000).is_some_and(|e| {
        e > now
            && ctx
                .int("dexalot.min_validity_ms")
                .is_ok_and(|ms| e - now >= ms)
    })
}

use crate::{
    gateway::{
        errors::{ErrorCode, GatewayError, GatewayResult},
        source_quote::{Preview, Request},
    },
    venues::bsc_dex::{ctx::Ctx, num, parts::Parts},
};
use alloy_primitives::{Address, U256};
use serde_json::json;
pub(super) fn fail(ctx: &Ctx, msg: &str) -> GatewayError {
    ctx.err(ErrorCode::BadResponse, msg)
}
#[derive(Clone, Copy)]
pub(super) struct Query {
    input: Address,
    output: Address,
    qty: U256,
    by_target: bool,
}
pub(super) async fn preview(p: &Parts, r: Request) -> GatewayResult<Preview> {
    let c = &p.ctx;
    let bad = || {
        fail(
            c,
            "Dexalot: укажите два разных физических актива и одну точную сумму",
        )
    };
    if r.input == r.output || r.market.is_some() || r.builder.is_some() {
        return Err(bad());
    }
    let chain = c.chain_id().ok_or_else(bad)?;
    let pairs = price::Pairs::load(c, chain).await?;
    let pair = pairs.find(r.input, r.output).ok_or_else(|| {
        fail(
            c,
            "Dexalot: физическая пара не зарегистрирована в этой сети",
        )
    })?;
    let dec = pair.precision(r.input).ok_or_else(bad)?;
    let exact = r.buy_amount_raw.is_some();
    let qty = if exact && r.amount_raw.is_none() && r.amount_tokens.is_none() {
        num::exact_amount(r.buy_amount_raw.as_deref(), None, 0)
    } else if !exact {
        num::exact_amount(r.amount_raw.as_deref(), r.amount_tokens.as_deref(), dec)
    } else {
        None
    };
    let q = Query {
        input: r.input,
        output: r.output,
        qty: qty.ok_or_else(bad)?,
        by_target: exact,
    };
    if let Some(f) = r.firm {
        return Box::pin(firm::report(
            p,
            q,
            f,
            r.trader,
            (dec, pair.precision(r.output).ok_or_else(bad)?),
        ))
        .await;
    }
    let answer = price::level(c, &pairs, pair, q, chain).await?;
    Ok(Preview {
        source: "dexalot".into(),
        input: q.input,
        output: q.output,
        spent_raw: answer.spent.to_string(),
        received_raw: answer.net.to_string(),
        unspent_raw: "0".into(),
        fee: None,
        block_hash: String::new(),
        block: None,
        expires: None,
        details: json!({"type":"indicative","chain_id":chain,"requested_pair":pair.name(),"price_source_pair":answer.physical.name(),"price_source_base":answer.physical.base_address,"price_source_quote":answer.physical.quote_address,"physical_alias":answer.physical!=pair}),
    })
}
