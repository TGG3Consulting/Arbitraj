//! Цена и выплата LambdaClass в следующем слоте конкретного строителя.
use super::prop_profile::{self, Profile};
use super::{
    Builder, Ctx, Head, Preview, Request, builder_snapshot, canonical, decimalsCall, rpc, slot,
};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::venues::bsc_dex::{exec::prop_abi::IProp, pools::prop_venue};
use alloy::sol_types::{SolCall, SolValue};
use alloy_primitives::{Address, Bytes, U64, keccak256};
use chrono::Utc;
use serde_json::{Value, json};

pub(in crate::venues::bsc_dex) async fn preview(
    ctx: &Ctx,
    id: &str,
    r: Request,
) -> GatewayResult<Preview> {
    let bad = |msg| ctx.err(ErrorCode::NotApplicable, msg);
    if r.buy_amount_raw.is_some() || r.input.is_zero() || r.output.is_zero() || r.input == r.output
    {
        return Err(bad(
            "Строитель PropAMM: нужен точный вход Ethereum и два контракта ERC20",
        ));
    }
    let rev = ctx.settings.generation();
    let chain_id =
        prop_profile::network(ctx).ok_or_else(|| bad("Слоты строителей недоступны в этой сети"))?;
    let profile = Profile::lookup(ctx, id).ok_or_else(|| {
        bad("Не настроен профиль PropAMM с ценовыми слотами и разрешёнными строителями")
    })?;
    let family = profile.family;
    let oracle = profile.oracle;
    let market = oracle.market;
    if r.market.is_some_and(|m| m != market) {
        return Err(bad("Выбран другой адрес рынка"));
    }
    let builder = r
        .builder
        .as_deref()
        .and_then(Builder::parse)
        .filter(|b| profile.builders.contains(b))
        .ok_or_else(|| bad("Укажите разрешённого для этого рынка строителя"))?;
    let payer = r
        .trader
        .filter(|v| !v.is_zero())
        .ok_or_else(|| bad("Укажите плательщика"))?;
    let floor = if let Some(t) = &r.firm {
        let owner = super::super::wallet::signer::address(&ctx.with_account(&t.wallet))?;
        if owner != payer || t.allow_trade || t.fee_tenths_bps.is_some() || t.maximum_raw.is_some()
        {
            return Err(bad("Условия не соответствуют проверке кошелька"));
        }
        Some(
            super::super::num::exact_amount(Some(&t.minimum_raw), None, 0)
                .filter(|v| !v.is_zero())
                .ok_or_else(|| bad("Укажите положительный минимум выплаты"))?,
        )
    } else {
        None
    };
    let (chain, h, snapshot): (U64, Head, Value) = tokio::try_join!(
        rpc::call(ctx, "eth_chainId", json!([])),
        rpc::call(ctx, "eth_getBlockByNumber", json!(["latest", false])),
        builder_snapshot(ctx, builder)
    )?;
    let stale = || {
        ctx.err(
            ErrorCode::StaleData,
            "Кадр строителя, родитель блока или настройки изменились",
        )
    };
    let f = oracle.frame(builder, &snapshot).ok_or_else(stale)?;
    let current = || {
        f.current(
            h.number.to(),
            h.timestamp.to(),
            Utc::now().timestamp().max(0) as u64,
        )
    };
    if chain != U64::from(chain_id) || !current() {
        return Err(stale());
    }
    let at = json!({"blockHash":h.hash,"requireCanonical":true});
    let (state, code, precision): (_, Bytes, _) = tokio::try_join!(
        prop_venue::state(ctx, family, market, (r.input, r.output), &at),
        rpc::call(ctx, "eth_getCode", json!([oracle.address, at])),
        canonical(ctx, h.hash, r.input, decimalsCall {})
    )?;
    let (roles, enabled) = state;
    if !enabled || code.is_empty() || keccak256(code) != oracle.hash {
        return Err(bad("Код оракула или допуск рынка изменился"));
    }
    let amount = super::super::num::exact_amount(
        r.amount_raw.as_deref(),
        r.amount_tokens.as_deref(),
        precision,
    )
    .filter(|v| !v.is_zero())
    .ok_or_else(|| bad("Недопустимый размер входа"))?;
    let call = |to: Address, data: Vec<u8>| json!({"from":payer,"to":to,"data":Bytes::from(data)});
    let decoded = || {
        ctx.err(
            ErrorCode::BadResponse,
            "Ответ не подтверждает выплату в слоте строителя",
        )
    };
    let (net, gas) = if let Some(minimum) = floor {
        let module = ctx.dex_addr("dex.router", family.id)?;
        let terms = super::prop_slot::terms(ctx, family, &roles, module, &h).await?;
        let amount_signed = alloy_primitives::I256::try_from(amount).map_err(|_| decoded())?;
        let leg =
            keccak256((h.hash, payer, market, r.input, r.output, amount, minimum).abi_encode());
        let order = super::prop_slot::Order {
            terms,
            payer,
            tokens: (r.input, r.output),
            specified: amount_signed,
            paid: amount,
            minimum,
            leg,
        }
        .proof(&h, &f, builder);
        let (_, credit, gas) = super::prop_slot::simulate(ctx, &h, &f, &order, None).await?;
        (credit, Some(gas))
    } else {
        let q = IProp::quoteVenueV1Call {
            venue: market,
            tokenIn: r.input,
            tokenOut: r.output,
            amountIn: amount,
        };
        let steps = slot::run(ctx, &h, &f, json!([call(roles.router(), q.abi_encode())])).await?;
        let [x] = steps.as_slice() else {
            return Err(decoded());
        };
        if x.status != U64::from(1) {
            return Err(decoded());
        }
        let res =
            IProp::quoteVenueV1Call::abi_decode_returns(&x.return_data).map_err(|_| decoded())?;
        if res.quotedVenue != market || res.amountOut.is_zero() {
            return Err(decoded());
        }
        (res.amountOut, None)
    };
    let (latest, snapshot): (Head, Value) = tokio::try_join!(
        rpc::call(ctx, "eth_getBlockByNumber", json!(["latest", false])),
        builder_snapshot(ctx, builder)
    )?;
    let new = oracle.frame(builder, &snapshot).ok_or_else(stale)?;
    if latest.hash != h.hash
        || !current()
        || rev != ctx.settings.generation()
        || (new.target, new.slot, &new.diff) != (f.target, f.slot, &f.diff)
    {
        return Err(stale());
    }
    Ok(Preview {
        source: id.to_owned(),
        input: r.input,
        output: r.output,
        spent_raw: amount.to_string(),
        received_raw: net.to_string(),
        unspent_raw: "0".into(),
        fee: None,
        block_hash: h.hash.to_string(),
        block: Some(h.number.to()),
        expires: Some(f.timestamp),
        details: json!({"type":if floor.is_some(){"wallet_simulation"}else{"indicative_price"},"market":market,"builder":builder.name(),"target_block":f.target,
            "slot":f.slot,"trader":payer,"owner_minimum_raw":floor.map(|v|v.to_string()),"gas_used":gas}),
    })
}
