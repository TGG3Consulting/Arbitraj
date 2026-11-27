//! Запрашивает один подписанный вариант для firm.rs и измеряет платёж по закреплённым условиям.
use super::{
    Query, client, fail,
    profile::{Config, Profile, Role},
    receipt::Saved,
    routing::Ticket,
    timely,
    wire::Order,
};
use crate::{
    gateway::errors::GatewayResult,
    venues::bsc_dex::{
        exec::signed_call::Erc20Call,
        parts::Parts,
        pools::exact_wallet_tokens,
        quote::white_state::{self, Head},
    },
};
use alloy_primitives::{Address, Bytes, U256, keccak256};
use serde_json::{Value, json};

#[derive(Clone, Copy)]
pub(super) struct Attempt {
    pub(super) chain: u64,
    pub(super) who: Address,
    pub(super) input: Address,
    pub(super) output: Address,
    pub(super) minimum: U256,
    pub(super) maximum: U256,
    pub(super) module: Option<Role>,
    pub(super) di: u8,
    pub(super) dout: u8,
    pub(super) generation: u64,
}
pub(super) async fn option(
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
    config.check_makers(c, a.module.is_some(), h).await?;
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
    p.dexalot
        .feed
        .hold_signed(c, (a.input, a.output), a.who, o.deadline());
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
            requested_output: q.by_target.then_some(q.qty),
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
