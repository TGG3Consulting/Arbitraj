//! Ручная подписанная котировка Clipper RFQ для собственного кошелька.
//! https://docs.sushi.com/api/blade/swagger
mod api;
mod sim;
#[cfg(test)]
mod tests;
#[allow(clippy::too_many_arguments)]
mod wire;
use super::{
    ctx::Ctx,
    node::{call, rpc},
    num,
    quote::white_state,
    wallet,
};
use crate::{
    gateway::{
        errors::{ErrorCode, GatewayResult},
        source_quote::{Preview, Request},
    },
    settings::scope::Scope,
};
use alloy_primitives::{Address, B256, Bytes, U64, U256, keccak256};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn roles(rows: &Value) -> Option<BTreeMap<Address, B256>> {
    let mut map = BTreeMap::new();
    for value in rows.as_array()? {
        let mut parts = value.as_str()?.split_whitespace();
        let addr: Address = parts.next()?.parse().ok()?;
        let hash: B256 = parts.next()?.parse().ok()?;
        if addr.is_zero()
            || hash.is_zero()
            || parts.next().is_some()
            || map.insert(addr, hash).is_some()
        {
            return None;
        }
    }
    (!map.is_empty()).then_some(map)
}
pub(super) async fn preview(ctx: &Ctx, r: Request) -> GatewayResult<Preview> {
    let bad = |s: &str| ctx.err(ErrorCode::BadResponse, s);
    let rev = ctx.settings.generation();
    let chain = ctx
        .chain_id()
        .filter(|c| *c > 0)
        .ok_or_else(|| bad("Clipper: укажите chainId сети этой площадки"))?;
    let terms = r
        .firm
        .as_ref()
        .ok_or_else(|| bad("Clipper: укажите свой кошелёк и минимум зачисления"))?;
    if r.input == r.output
        || r.builder.is_some()
        || r.buy_amount_raw.is_some()
        || r.amount_tokens.is_some()
        || terms.maximum_raw.is_some()
        || terms.allow_trade
        || terms.fee_tenths_bps.is_some()
    {
        return Err(bad("Clipper: требуется ручной запрос точной суммы продажи"));
    }
    let owner = wallet::signer::address(&ctx.with_account(&terms.wallet))?;
    if owner.is_zero() || r.trader != Some(owner) {
        return Err(bad(
            "Clipper: адрес плательщика отличается от выбранного кошелька",
        ));
    }
    let amount = num::exact_amount(r.amount_raw.as_deref(), None, 0)
        .ok_or_else(|| bad("Clipper: положительный расход в минимальных единицах обязателен"))?;
    let minimum = num::exact_amount(Some(&terms.minimum_raw), None, 0)
        .ok_or_else(|| bad("Clipper: положительный минимум в единицах выхода обязателен"))?;
    let map = ctx
        .settings
        .get_exact("clipper.pools", &Scope::Venue(ctx.place.id.to_string()))
        .as_ref()
        .and_then(roles)
        .ok_or_else(|| bad("Clipper: заполните clipper.pools для этой сети"))?;
    let pool = r
        .market
        .or_else(|| {
            if map.len() == 1 {
                map.keys().next().copied()
            } else {
                None
            }
        })
        .ok_or_else(|| bad("Clipper: укажите один из настроенных пулов этой сети"))?;
    let hash = map
        .get(&pool)
        .ok_or_else(|| bad("Clipper: нет этого пула среди разрешённых адресов сети"))?;
    let api = api::Api::new(ctx, chain)?;
    let wrapper = ctx.addr("dex.native_wrapped")?;
    let norm = |a: Address| if a.is_zero() { wrapper } else { a };
    let [input, output] = [norm(r.input), norm(r.output)];
    if wrapper.is_zero() || input == output {
        return Err(bad("Clipper: физические активы должны быть различны"));
    }
    let pool_query = [("pool_address", pool.to_string())];
    let (net, book): (U64, Value) = tokio::try_join!(
        rpc::call(ctx, "eth_chainId", json!([])),
        api.get(ctx, "pool", &pool_query)
    )?;
    if net.to::<u64>() != chain {
        return Err(bad("Clipper: узел другой сети"));
    }
    let [a, b] = api::instruments(&book, chain, pool, [input, output])
        .ok_or_else(|| bad("Blade: в ответе нет действующего пула с обеими валютами этой сети"))?;
    let args = [
        ("pool_address", pool.to_string()),
        ("input_amount", amount.to_string()),
        ("input_asset_symbol", a),
        ("output_asset_symbol", b),
        ("sender_address", owner.to_string()),
        ("destination_address", owner.to_string()),
        ("time_in_seconds", "30".into()),
    ];
    let q: wire::Quote = serde_json::from_value(api.get(ctx, "quote-sign", &args).await?)
        .map_err(|_| bad("Blade: нет всех полей оферты и подписи"))?;
    let head = white_state::head(ctx, "latest").await?;
    let at = head.selector();
    let checked = tokio::try_join!(
        rpc::call::<Bytes>(ctx, "eth_getCode", json!([pool, at])),
        call::canonical(ctx, &at, pool, wire::DESIGNATED_SIGNERCall {}),
        call::canonical(ctx, &at, pool, wire::WRAPPER_CONTRACTCall {}),
        call::canonical(ctx, &at, pool, wire::isTokenCall { t: input }),
        call::canonical(ctx, &at, pool, wire::isTokenCall { t: output })
    )?;
    if checked.0.is_empty()
        || keccak256(&checked.0) != *hash
        || checked.2 != wrapper
        || !checked.3
        || !checked.4
    {
        return Err(bad(
            "Clipper: код пула, обёртка или текущий состав валют изменились",
        ));
    }
    let offer = wire::Offer {
        chain,
        pool,
        owner,
        input,
        output,
        amount,
        received: num::exact_amount(Some(&q.output_amount), None, 0)
            .ok_or_else(|| bad("Blade: неверная выплата"))?,
        until: U256::from_str_radix(&q.good_until, 10)
            .map_err(|_| bad("Blade: неверный срок подписанного предложения"))?,
    };
    let current = || {
        offer.expires() > chrono::Utc::now().timestamp().max(0) as u64
            && rev == ctx.settings.generation()
    };
    if !current()
        || offer.expires()
            < super::check::simulate::Base::next_time(Some(chain), head.timestamp.to())
        || !offer.valid(&q, checked.1)
    {
        return Err(bad(
            "Clipper: подпись, срок или условия не соответствуют заявке и designated signer пула",
        ));
    }
    let (received, gas) = sim::Physical {
        input: r.input,
        output: r.output,
    }
    .measure(ctx, head, &offer, &q, minimum)
    .await?;
    let canonical = white_state::head(ctx, &format!("{:#x}", head.number)).await?;
    if !current() || canonical.hash != head.hash {
        return Err(ctx.err(
            ErrorCode::StaleData,
            "Clipper: котировка, блок или настройки сменились во время проверки",
        ));
    }
    Ok(Preview {
        source: "clipper".into(),
        input: r.input,
        output: r.output,
        spent_raw: amount.to_string(),
        received_raw: received.to_string(),
        unspent_raw: "0".into(),
        fee: None,
        block_hash: format!("{:#x}", head.hash),
        block: Some(head.number.to()),
        expires: Some(offer.expires()),
        details: json!({"type":"wallet_simulation","gas_used":gas,"owner_minimum_raw":minimum.to_string(),"signed_output_raw":offer.received.to_string(),
        "signer":checked.1,"pool":pool,"digest":offer.digest(),"payer":owner}),
    })
}
