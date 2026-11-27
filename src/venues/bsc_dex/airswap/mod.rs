//! AirSwap RFQ. Obtain signed terms only from selected makers and replay the actual payer's call.
mod client;
mod indicative;
mod profile;
pub(super) mod receipt;
pub(super) mod routing;
#[cfg(test)]
mod tests;
mod trade;
mod wire;
pub(super) use routing::Book;
pub(super) use trade::trade;
pub(super) const LABEL: &str = "AirSwap RFQ";
use super::{
    ctx::Ctx, exec::signed_call::Erc20Call, node::call, num, parts::Parts,
    pools::exact_wallet_tokens, quote::white_state, wallet,
};
use crate::gateway::{
    errors::{ErrorCode, GatewayError, GatewayResult},
    source_quote::{Preview, Request},
};
use alloy_primitives::{Address, U256, keccak256};
use client::{Maker, Query};
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
async fn options(
    p: &Parts,
    q: Query,
    limits: Limits,
    who: Address,
    head: white_state::Head,
) -> GatewayResult<Vec<routing::Ticket>> {
    let c = &p.ctx;
    let profiles = profile::Profile::candidates(c);
    let peers = Maker::endpoints(c);
    if peers.is_empty() {
        return Err(no(c, "AirSwap: не указан HTTPS-сервер maker"));
    }
    let generation = c.settings.generation();
    let mut out = Vec::new();
    let (di, dout) = tokio::try_join!(
        p.tokens.decimals(c, q.input),
        p.tokens.decimals(c, q.output)
    )?;
    let ms = c.int("airswap.min_validity_ms")?;
    use futures_util::{StreamExt, stream};
    let deadline = tokio::time::Instant::now() + c.timeout()?;
    let mut jobs = Vec::new();
    for scope in profiles {
        for maker in &peers {
            jobs.push(option(
                c,
                q,
                limits,
                head,
                (di, dout),
                scope,
                maker,
                (generation, who, ms),
            ));
        }
    }
    let mut jobs = stream::iter(jobs).buffer_unordered(4);
    let mut last = None;
    while let Ok(Some(result)) = tokio::time::timeout_at(deadline, jobs.next()).await {
        match result {
            Ok(mut rows) => out.append(&mut rows),
            Err(e) => last = Some(e),
        }
    }
    if c.settings.generation() != generation {
        return Err(no(c, "AirSwap: настройки изменились"));
    }
    if out.is_empty() {
        return Err(last.unwrap_or_else(|| {
            no(
                c,
                "AirSwap: нет оплачиваемой заявки в этих пределах и сроке",
            )
        }));
    }
    Ok(out)
}
#[allow(clippy::too_many_arguments)]
async fn option(
    c: &Ctx,
    q: Query,
    limits: Limits,
    head: white_state::Head,
    (di, dout): (u8, u8),
    scope: profile::Profile,
    maker: &Maker,
    (generation, who, ms): (u64, Address, u64),
) -> GatewayResult<Vec<routing::Ticket>> {
    let Some(prepared) = prepare(c, q, scope, maker, who, head, None).await? else {
        return Ok(vec![]);
    };
    signed(
        c,
        q,
        limits,
        head,
        (di, dout),
        scope,
        maker,
        (generation, who, ms),
        prepared,
    )
    .await
}
async fn prepare(
    c: &Ctx,
    q: Query,
    scope: profile::Profile,
    maker: &Maker,
    who: Address,
    head: white_state::Head,
    required: Option<U256>,
) -> GatewayResult<Option<(alloy_primitives::B256, U256)>> {
    if scope.module.is_none() {
        exact_wallet_tokens::check(c, "airswap", q.input, q.output, &head.selector()).await?;
    }
    let terms = scope.check(c, head, who).await?;
    if let Some(spend) = required {
        use super::pools::dagrow;
        let at = head.selector();
        let (balance, grant) = tokio::try_join!(
            dagrow::balance(c, q.input, who, &at),
            dagrow::allowance(c, q.input, who, scope.target(), &at)
        )?;
        if spend.is_zero() || spend > balance || spend > grant {
            return Ok(None);
        }
    }
    if !maker.supports(c, q, scope).await? {
        return Ok(None);
    }
    Ok(Some(terms))
}
#[allow(clippy::too_many_arguments)]
async fn signed(
    c: &Ctx,
    q: Query,
    limits: Limits,
    head: white_state::Head,
    (di, dout): (u8, u8),
    scope: profile::Profile,
    maker: &Maker,
    (generation, who, ms): (u64, Address, u64),
    (domain, fee): (alloy_primitives::B256, U256),
) -> GatewayResult<Vec<routing::Ticket>> {
    let mut out = Vec::new();
    let sender = scope.sender(who);
    let order = maker.order(c, q, scope, sender, ms).await?;
    if !timely(c, order.expiry)
        || order.amount > limits.maximum
        || order.fee != fee
        || order.maker == who
        || order.maker == sender
        || order.maker == scope.exchange.address
    {
        return Ok(out);
    }
    let at = head.selector();
    let Ok(authorized) = call::canonical(
        c,
        &at,
        scope.exchange.address,
        wire::ISwap::authorizedCall {
            signerWallet: order.maker,
        },
    )
    .await
    else {
        return Ok(out);
    };
    let signatory = if authorized.is_zero() {
        order.maker
    } else {
        authorized
    };
    if !matches!(
        call::canonical(
            c,
            &at,
            scope.exchange.address,
            wire::ISwap::nonceUsedCall {
                signatory,
                nonce: order.nonce
            }
        )
        .await,
        Ok(false)
    ) {
        return Ok(out);
    }
    for any in [false, true] {
        let signed_for = if any { Address::ZERO } else { sender };
        if order.sender.is_some_and(|s| s != signed_for)
            || (scope.module.is_none() && order.quoted < limits.minimum)
        {
            continue;
        }
        let minimum = if scope.module.is_none() {
            order.quoted
        } else {
            limits.minimum
        };
        let mut data = order.data(who, scope.module.is_some(), any, minimum);
        let mut proof = receipt::Saved {
            call: Erc20Call {
                chain: scope.chain,
                payer: who,
                module: scope.target(),
                calldata: keccak256(&data),
            },
            profile: scope,
            maker: order.maker,
            nonce: order.nonce,
            digest: order.digest(domain, signed_for),
            expiry: order.expiry,
            input: q.input,
            output: q.output,
            amount: order.amount,
            quoted: order.quoted,
            fee,
            minimum,
        };
        if let Ok((spent, received, gas)) = proof.measure(c, head, &data).await
            && spent <= limits.maximum
            && received >= limits.minimum
            && timely(c, proof.expiry)
        {
            let gas = if scope.module.is_some() && received > minimum {
                proof.minimum = received;
                data = order.data(who, true, any, received);
                proof.call.calldata = keccak256(&data);
                let Ok((paid, credit, g)) = proof.measure(c, head, &data).await else {
                    continue;
                };
                if (paid, credit) != (spent, received) {
                    continue;
                }
                g
            } else {
                gas
            };
            out.push(routing::Ticket {
                signatory,
                proof,
                data,
                spent,
                received,
                gas,
                generation,
                di,
                dout,
            });
        }
    }

    Ok(out)
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
        let tickets = options(&account, q, limits, owner, head).await?;
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
