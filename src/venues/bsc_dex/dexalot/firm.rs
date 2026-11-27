use super::{
    Query, client, enabled, fail,
    profile::{Config, Profile, Role},
    receipt::Saved,
    routing::Ticket,
    timely,
    wire::Order,
};
use crate::{
    gateway::{
        errors::GatewayResult,
        source_quote::{FirmTerms, Preview},
    },
    venues::bsc_dex::{
        exec::signed_call::Erc20Call,
        num,
        parts::Parts,
        pools::{dagrow, exact_wallet_tokens},
        quote::white_state::{self, Head},
        wallet,
    },
};
use alloy_primitives::{Address, Bytes, U256, keccak256};
use serde_json::{Value, json};
use std::sync::Arc;
pub(super) async fn report(
    p: &Parts,
    q: Query,
    f: FirmTerms,
    trader: Option<Address>,
    decs: (u8, u8),
) -> GatewayResult<Preview> {
    let account = p.bound(&f.wallet);
    let ctx = &account.ctx;
    ctx.bound()?;
    let who = wallet::signer::address(ctx)?;
    let bad = || {
        fail(
            ctx,
            "Dexalot: проверьте разрешение, сумму, порог, сроки и собственный кошелёк",
        )
    };
    if !enabled(ctx) || trader != Some(who) || f.fee_tenths_bps.is_some() {
        return Err(bad());
    }
    let maximum = match (q.by_target, f.maximum_raw.as_deref()) {
        (false, None) => q.qty,
        (true, Some(x)) => num::exact_amount(Some(x), None, 0).ok_or_else(bad)?,
        _ => return Err(bad()),
    };
    let minimum = num::exact_amount(Some(&f.minimum_raw), None, 0)
        .ok_or_else(bad)?
        .max(if q.by_target { q.qty } else { U256::ONE });
    let chain = ctx.chain_id().ok_or_else(bad)?;
    let wrapped = ctx.addr("dex.native_wrapped")?;
    let map = |a: Address| if a.is_zero() { wrapped } else { a };
    let (input, output) = (map(q.input), map(q.output));
    if input == output {
        return Err(bad());
    }
    let (di, dout) = tokio::try_join!(
        account.tokens.decimals(ctx, input),
        account.tokens.decimals(ctx, output)
    )?;
    if (di, dout) != decs {
        return Err(bad());
    }
    let gen_before = ctx.settings.generation();
    let config = Config::read(ctx)?;
    let h = white_state::head(ctx, "latest").await?;
    let bal = dagrow::balance(ctx, input, who, &h.selector()).await?;
    if (!q.by_target && bal < q.qty) || bal.is_zero() {
        return Err(bad());
    }
    let mut choices = Vec::new();
    for m in &config.modules {
        if config.ready(ctx, who, Some(*m), h).await.is_ok()
            && dagrow::allowance(ctx, input, who, m.addr, &h.selector())
                .await
                .is_ok_and(|v| !v.is_zero() && (q.by_target || v >= q.qty))
        {
            choices.push(Some(*m));
        }
    }
    // A bare immutable maker can be considered for an exact ERC20 pair, a zero slippage flag and the owner's own payee.
    if !q.input.is_zero()
        && !q.output.is_zero()
        && exact_wallet_tokens::check(ctx, "dexalot", input, output, &h.selector())
            .await
            .is_ok()
    {
        for m in &config.makers {
            if m.proxy.is_none()
                && dagrow::allowance(ctx, input, who, m.addr, &h.selector())
                    .await
                    .is_ok_and(|v| !v.is_zero() && (q.by_target || v >= q.qty))
            {
                choices.push(None);
                break;
            }
        }
    }
    if choices.is_empty() {
        return Err(fail(
            ctx,
            "Dexalot: нет настроенного модуля или неизменяемого maker с разрешением от владельца",
        ));
    }
    let mut tickets = Vec::new();
    let mut last = None;
    for m in choices {
        if ctx.settings.generation() != gen_before {
            break;
        }
        let scope = Attempt {
            chain,
            who,
            input,
            output,
            minimum,
            maximum,
            module: m,
            di,
            dout,
            generation: gen_before,
        };
        match option(&account, &config, q, h, scope).await {
            Ok(t) => tickets.push(Arc::new(t)),
            Err(e) => last = Some(e),
        }
    }
    if ctx.settings.generation() != gen_before {
        return Err(bad());
    }
    let best = tickets
        .iter()
        .min_by(|a, b| {
            if q.by_target {
                a.spent.cmp(&b.spent).then(a.gas.cmp(&b.gas))
            } else {
                b.received.cmp(&a.received).then(a.gas.cmp(&b.gas))
            }
        })
        .cloned()
        .ok_or_else(|| last.unwrap_or_else(bad))?;
    let report = Preview {
        source: "dexalot".into(),
        input,
        output,
        spent_raw: best.spent.to_string(),
        received_raw: best.received.to_string(),
        unspent_raw: maximum.checked_sub(best.spent).ok_or_else(bad)?.to_string(),
        fee: None,
        block_hash: format!("{:#x}", best.head.hash),
        block: Some(best.head.number.to()),
        expires: Some(best.proof.expiry),
        details: json!({"type":"wallet_simulation","physical_source":q.input,"physical_destination":q.output,"maker":best.proof.profile.maker.addr,"nonce":best.proof.nonce,"signed_digest":best.proof.digest,"signed_out":best.proof.quoted.to_string(),"minimum":best.proof.minimum.to_string(),"gas":best.gas,"candidates":tickets.len(),"allow_trade":f.allow_trade}),
    };
    if f.allow_trade {
        for t in tickets {
            p.dexalot.put(t)
        }
    }
    Ok(report)
}
#[derive(Clone, Copy)]
struct Attempt {
    chain: u64,
    who: Address,
    input: Address,
    output: Address,
    minimum: U256,
    maximum: U256,
    module: Option<Role>,
    di: u8,
    dout: u8,
    generation: u64,
}
async fn option(
    p: &Parts,
    config: &Config,
    q: Query,
    h: Head,
    a: Attempt,
) -> GatewayResult<Ticket> {
    let c = &p.ctx;
    let bad = || {
        fail(
            c,
            "Dexalot: подписанные физические активы, плательщик, срок или чистый результат не прошли проверку",
        )
    };
    config.ready(c, a.who, a.module, h).await?;
    let actor = a.module.map_or(a.who, |v| v.addr);
    let mut body = json!({"chainid":a.chain,"takerAsset":q.input,"makerAsset":q.output,"userAddress":a.who,"executor":actor});
    body[if q.by_target {
        "makerAmount"
    } else {
        "takerAmount"
    }] = json!(q.qty.to_string());
    let raw: Value = client::request(c, "firm", &[], Some(&body)).await?;
    let o = Order::parse(&raw["order"]).ok_or_else(bad)?;
    let signature: Bytes = raw["signature"]
        .as_str()
        .and_then(|x| x.parse().ok())
        .filter(|b: &Bytes| !b.is_empty())
        .ok_or_else(bad)?;
    let maker = *config
        .makers
        .iter()
        .find(|r| r.addr == o.maker)
        .ok_or_else(bad)?;
    let profile = Profile {
        router: config.router,
        maker,
        module: a.module,
    };
    if !timely(c, o.deadline())
        || (o.takerAsset, o.makerAsset) != (q.input, q.output)
        || o.makerAmount < a.minimum
        || (!q.by_target && o.takerAmount < q.qty)
        || ![actor, a.who].contains(&o.taker)
        || ![actor, a.who].contains(&o.payee())
        || (o.taker != actor && o.payee() != actor)
        || (q.output.is_zero() && o.payee() != actor)
    {
        return Err(bad());
    }
    if a.module.is_none()
        && (maker.proxy.is_some() || !o.constant_output() || o.payee() != a.who || o.taker != a.who)
    {
        return Err(bad());
    }
    let fresh = white_state::head(c, "latest").await?;
    config.ready(c, a.who, a.module, fresh).await?;
    let digest = profile.authorize(c, &o, signature.clone(), fresh).await?;
    if a.module.is_none() {
        exact_wallet_tokens::check(c, "dexalot", q.input, q.output, &fresh.selector()).await?;
    }
    let real_cap = o.takerAmount.min(a.maximum);
    let make = |partial, minimum| {
        let data = o.data(
            signature.clone(),
            a.module.is_some(),
            real_cap,
            minimum,
            partial,
        );
        let call = Erc20Call {
            chain: a.chain,
            payer: a.who,
            module: profile.target(),
            calldata: keccak256(&data),
        };
        let proof = Saved {
            call,
            profile,
            nonce: o.nonceAndMeta,
            digest,
            expiry: o.deadline(),
            input: a.input,
            output: a.output,
            source: q.input,
            destination: q.output,
            taker: o.taker,
            payee: o.payee(),
            amount: real_cap,
            quoted: o.makerAmount,
            minimum,
            partial,
        };
        (data, proof)
    };
    let mut best = None;
    for partial in [false, true] {
        if (!partial && o.takerAmount > real_cap) || (partial && a.module.is_none()) {
            continue;
        }
        let minimum = if a.module.is_none() {
            o.makerAmount
        } else {
            a.minimum
        };
        let (mut data, mut proof) = make(partial, minimum);
        let Ok((spent, net, mut gas)) = proof.measure(c, fresh, &data).await else {
            continue;
        };
        if net < a.minimum || spent > a.maximum || (!q.by_target && spent != q.qty) {
            continue;
        }
        if a.module.is_some() && net > minimum {
            (data, proof) = make(partial, net);
            let Ok((x, y, g)) = proof.measure(c, fresh, &data).await else {
                continue;
            };
            if (x, y) != (spent, net) {
                continue;
            }
            gas = g;
        }
        let t = Ticket {
            proof,
            data,
            spent,
            received: net,
            gas,
            generation: a.generation,
            di: a.di,
            dout: a.dout,
            head: fresh,
        };
        if best.as_ref().is_none_or(|b: &Ticket| {
            if q.by_target {
                (spent, gas) < (b.spent, b.gas)
            } else {
                (net, std::cmp::Reverse(gas)) > (b.received, std::cmp::Reverse(b.gas))
            }
        }) {
            best = Some(t);
        }
    }
    best.filter(|v| timely(c, v.proof.expiry)).ok_or_else(bad)
}
