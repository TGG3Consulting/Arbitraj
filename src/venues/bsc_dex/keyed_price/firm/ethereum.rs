//! Допуск одной котировки выбранным кошельком, без отправки транзакции.
use super::{RequestApi, Ticket, bad, wire};
use crate::{
    gateway::{
        errors::{ErrorCode, GatewayResult},
        http,
        source_quote::Preview,
    },
    venues::bsc_dex::{
        exec::signed_call::Erc20Call,
        node::{abi::IERC20, call},
        parts::Parts,
        quote::white_state,
        zeroex,
    },
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Bytes, U256, keccak256};
use serde_json::{Value, json};

pub(in crate::venues::bsc_dex::keyed_price) async fn admit(
    p: &Parts,
    t: Ticket,
    mut api: RequestApi,
    decimals: (u8, u8),
) -> GatewayResult<Preview> {
    let ctx = &p.ctx;
    ctx.bound()?;
    if !zeroex::enabled(ctx)
        || api.auth.is_none()
        || api.url.origin().ascii_serialization() != "https://api.0x.org"
    {
        return Err(ctx.err(
            ErrorCode::NotConfigured,
            "Для допуска нужен официальный API и включённая торговля 0x на Ethereum",
        ));
    }
    let module = zeroex::module(ctx)?;
    let head = white_state::head(ctx, "latest").await?;
    let at = head.selector();
    let (balance, permission) = tokio::try_join!(
        call::canonical(ctx, &at, t.input, IERC20::balanceOfCall { owner: t.owner }),
        call::canonical(
            ctx,
            &at,
            t.input,
            IERC20::allowanceCall {
                owner: t.owner,
                spender: module
            }
        )
    )?;
    if permission < t.amount || balance < t.amount {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            "0x: у выбранного кошелька недостаточно средств или разрешения модулю",
        ));
    }
    api.url.query_pairs_mut().extend_pairs([
        ("chainId", api.chain.to_string()),
        ("taker", module.to_string()),
        ("txOrigin", t.owner.to_string()),
        ("recipient", t.owner.to_string()),
        ("sellToken", t.input.to_string()),
        ("buyToken", t.output.to_string()),
        ("sellAmount", t.amount.to_string()),
    ]);
    let key = api.auth.take().ok_or_else(|| bad(ctx, "Нет ключа 0x"))?;
    let request = api
        .client
        .get(api.url)
        .header(api.header, key)
        .header("0x-version", "v2");
    let (status, body) = http::exchange(request, ctx.limits()?)
        .await
        .map_err(|_| bad(ctx, "0x: не получен ответ на индивидуальную заявку"))?;
    if !status.is_success() {
        return Err(bad(ctx, &format!("0x HTTP {}", status.as_u16())));
    }
    let data: Value = serde_json::from_str(&body)
        .map_err(|_| bad(ctx, "0x: неполный ответ индивидуальной заявки"))?;
    let (original, mut details) = wire::zeroex(&t, &data)
        .ok_or_else(|| bad(ctx, "0x: условия ответа отличаются от запроса владельца"))?;
    let settler = wire::settler(&t, &original)
        .ok_or_else(|| bad(ctx, "0x: не распознан контракт исполнения"))?;
    let height = data["blockNumber"]
        .as_str()
        .and_then(|x| x.parse::<u64>().ok())
        .filter(|h| *h > 0)
        .ok_or_else(|| bad(ctx, "0x: нет блока исходной котировки"))?;
    let src = white_state::head(ctx, &format!("{height:#x}")).await?;
    let latest = white_state::head(ctx, "latest").await?;
    if src.number.to::<u64>() != height || height > latest.number.to::<u64>() {
        return Err(bad(ctx, "0x: не подтверждён исходный блок"));
    }
    let age = ctx.int("feed.stale_after_ms")?;
    let expiry = src
        .timestamp
        .to::<u64>()
        .checked_add(age / 1000)
        .ok_or_else(|| bad(ctx, "Некорректный срок котировки 0x"))?;
    let minimum = data["minBuyAmount"]
        .as_str()
        .and_then(|x| x.parse::<U256>().ok())
        .filter(|v| *v >= t.minimum)
        .ok_or_else(|| bad(ctx, "0x: не подтверждён минимум"))?;
    if !zeroex::current(expiry) {
        return Err(ctx.err(ErrorCode::NoRoute, "0x: данные котировки устарели"));
    }
    zeroex::verify(ctx, &latest.selector(), module, settler).await?;
    let action: Bytes = zeroex::receipt::IWallet::settleCall {
        input: t.input,
        output: t.output,
        amount: t.amount,
        minimum,
        expiry: U256::from(expiry),
        original: original.clone(),
    }
    .abi_encode()
    .into();
    let id = keccak256(&action);
    let proof = zeroex::receipt::Saved::new(
        Erc20Call {
            chain: api.chain,
            payer: t.owner,
            module,
            calldata: id,
        },
        keccak256(original),
        settler,
        (t.input, t.output),
        t.amount,
        minimum,
        expiry,
    );
    let (received, gas) = proof.measure(ctx, latest, &action).await?;
    if !zeroex::current(expiry) || t.generation != ctx.settings.generation() {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            "0x: срок или настройки изменились во время проверки",
        ));
    }
    details["type"] = json!("wallet_simulation");
    details["gas_used"] = json!(gas);
    details["owner_minimum_raw"] = json!(t.minimum.to_string());
    details["owner_admitted"] = json!(true);
    details["ticket"] = json!(id);
    p.zeroex.put(zeroex::routing::Ticket {
        proof,
        data: action,
        gas,
        generation: t.generation,
        di: decimals.0,
        dout: decimals.1,
    });
    Ok(Preview {
        source: "zeroex".into(),
        input: t.input,
        output: t.output,
        spent_raw: t.amount.to_string(),
        received_raw: received.to_string(),
        unspent_raw: "0".into(),
        fee: None,
        block_hash: format!("{:#x}", latest.hash),
        block: Some(latest.number.to()),
        expires: Some(expiry),
        details,
    })
}
