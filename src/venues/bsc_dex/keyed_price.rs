//! Ручная оценка официальных API Kipseli и 0x для указанного размера.
//! Служебный ключ передаётся только исходному HTTPS-адресу соответствующего API.
mod firm;
mod kipseli;
use super::{
    ctx::Ctx,
    node::{abi::IERC20, call},
    num,
    quote::white_state,
};
use crate::gateway::{
    errors::{ErrorCode, GatewayResult},
    http,
    source_quote::{Preview, Request},
};
use alloy_primitives::{Address, U256};
use reqwest::{Client, header::HeaderValue};
use serde_json::{Value, json};

fn endpoint(base: &str, official: &str, path: &str) -> Option<(reqwest::Url, bool)> {
    let mut url = reqwest::Url::parse(base).ok()?;
    if url.scheme() != "https"
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let send_key = url.origin().ascii_serialization() == official;
    url.path_segments_mut()
        .ok()?
        .pop_if_empty()
        .extend(path.split('/'));
    Some((url, send_key))
}

pub(super) async fn preview(ctx: &Ctx, source: &str, req: Request) -> GatewayResult<Preview> {
    let invalid = |m: &str| ctx.err(ErrorCode::BadResponse, m);
    let chain = ctx
        .chain_id()
        .ok_or_else(|| invalid("Не задан chainId площадки"))?;
    let (setting, key, header, origin, path) = match source {
        "kipseli" => (
            "kipseli.base_url",
            "kipseli.api_key",
            "x-api-key",
            "https://propamm.kipseli.win",
            "v2/price",
        ),
        "zeroex" => (
            "zeroex.base_url",
            "zeroex.api_key",
            "0x-api-key",
            "https://api.0x.org",
            "swap/allowance-holder/price",
        ),
        _ => return Err(ctx.err(ErrorCode::NotApplicable, "Неизвестный источник")),
    };
    let base = ctx.text(setting)?;
    let path = if req.firm.is_some() {
        if source == "kipseli" {
            "v2/swap/sign"
        } else {
            "swap/allowance-holder/quote"
        }
    } else {
        path
    };
    let (mut url, official) = endpoint(&base, origin, path)
        .ok_or_else(|| invalid("Для источника нужен корректный адрес HTTPS"))?;
    if req.input == req.output
        || req.input.is_zero()
        || req.output.is_zero()
        || req.market.is_some()
        || req.builder.is_some()
    {
        return Err(invalid(
            "Укажите два разных контракта ERC20 и сумму продажи",
        ));
    }
    let auth = if official {
        let credential = ctx.vault.get(key).map_err(|_| {
            ctx.err(
                ErrorCode::NotConfigured,
                format!("Откройте хранилище и задайте {key}"),
            )
        })?;
        if credential.trim().is_empty() {
            return Err(ctx.err(ErrorCode::NotConfigured, format!("Заполните {key}")));
        }
        let mut auth = HeaderValue::from_str(credential.as_str())
            .map_err(|_| invalid("Некорректный заголовок учётных данных"))?;
        auth.set_sensitive(true);
        Some(auth)
    } else {
        None
    };
    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .build()
        .map_err(|_| invalid("Не создан клиент HTTP"))?;
    let latest = white_state::head(ctx, "latest").await?;
    let at = latest.selector();
    let (dec_in, dec_out) = tokio::try_join!(
        call::canonical(ctx, &at, req.input, IERC20::decimalsCall {}),
        call::canonical(ctx, &at, req.output, IERC20::decimalsCall {})
    )?;
    let amount = num::exact_amount(
        req.amount_raw.as_deref(),
        req.amount_tokens.as_deref(),
        dec_in,
    )
    .ok_or_else(|| invalid("Укажите положительную сумму с точностью токена"))?;
    let mut wire = if let Some(terms) = req.firm.as_ref() {
        let ticket = firm::Ticket::new(ctx, source, &req, terms, amount)?;
        let config = firm::RequestApi {
            client,
            auth,
            header,
            url,
            chain,
        };
        return ticket.preview(ctx, config).await;
    } else {
        url.query_pairs_mut()
            .append_pair("chainId", &chain.to_string());
        if source == "zeroex" {
            let trader = req
                .trader
                .filter(|a| !a.is_zero())
                .ok_or_else(|| invalid("Укажите публичный адрес плательщика 0x"))?;
            url.query_pairs_mut().extend_pairs([
                ("sellToken", req.input.to_string()),
                ("buyToken", req.output.to_string()),
                ("sellAmount", amount.to_string()),
                ("taker", trader.to_string()),
            ]);
        }
        client.get(url)
    };
    if let Some(auth) = auth {
        wire = wire.header(header, auth);
    }
    if source == "zeroex" {
        wire = wire.header("0x-version", "v2");
    }
    let (status, body) = http::exchange(wire, ctx.limits()?)
        .await
        .map_err(|_| invalid("Не получен полный ответ источника цены"))?;
    if !status.is_success() {
        return Err(invalid(&format!("{source} HTTP {}", status.as_u16())));
    }
    let book: Value =
        serde_json::from_str(&body).map_err(|_| invalid("Некорректный JSON источника цены"))?;
    let (height, paid, got, details) = if source == "kipseli" {
        if book["chainId"].as_u64() != Some(chain) {
            return Err(invalid("Другая сеть в стакане Kipseli"));
        }
        let (paid, got) = kipseli::amounts(&book, req.input, req.output, dec_in, dec_out, amount)
            .ok_or_else(|| {
            ctx.err(
                ErrorCode::NoRoute,
                "Нет положительного выхода на уровнях Kipseli",
            )
        })?;
        (
            book["blockNumber"].as_u64(),
            paid,
            got,
            json!({"type":"marginal_depth","signed":false,"fee_known":false}),
        )
    } else {
        let (got, details) = zeroex(&book, req.input, req.output, amount)
            .ok_or_else(|| invalid("Маршрут или сумма 0x отличаются от заявки"))?;
        (
            book["blockNumber"].as_str().and_then(|s| s.parse().ok()),
            amount,
            got,
            details,
        )
    };
    let block = height
        .filter(|h| *h > 0)
        .ok_or_else(|| invalid("Нет номера блока источника"))?;
    let head = white_state::head(ctx, &format!("{block:#x}")).await?;
    if head.number.to::<u64>() != block
        || (source == "kipseli" && book["blockTime"].as_u64() != Some(head.timestamp.to::<u64>()))
    {
        return Err(invalid(
            "Высота или время блока источника не совпали с узлом",
        ));
    }
    Ok(Preview {
        source: source.into(),
        input: req.input,
        output: req.output,
        spent_raw: paid.to_string(),
        received_raw: got.to_string(),
        unspent_raw: (amount - paid).to_string(),
        fee: None,
        block_hash: format!("{:#x}", head.hash),
        block: Some(block),
        expires: None,
        details,
    })
}
fn zeroex(book: &Value, input: Address, output: Address, amount: U256) -> Option<(U256, Value)> {
    let raw = |key: &str| num::exact_amount(book[key].as_str(), None, 0);
    if book["liquidityAvailable"] != true
        || raw("sellAmount") != Some(amount)
        || book["sellToken"].as_str()?.parse::<Address>().ok()? != input
        || book["buyToken"].as_str()?.parse::<Address>().ok()? != output
    {
        return None;
    }
    let got = raw("buyAmount")?;
    let floor = raw("minBuyAmount")?;
    if floor > got {
        return None;
    }
    let mut legs = Vec::new();
    let mut named = 0;
    for r in book["route"]["fills"].as_array()? {
        let source = r["source"].as_str()?;
        let p = r["proportionBps"].as_str()?.parse::<u16>().ok()?;
        let a = r["from"].as_str()?.parse::<Address>().ok()?;
        let b = r["to"].as_str()?.parse::<Address>().ok()?;
        if a == b || a.is_zero() || b.is_zero() || p == 0 || p > 10000 || source.is_empty() {
            return None;
        }
        named += usize::from(source == "0x_RFQ");
        legs.push(json!({"source":source,"from":a,"to":b,"proportion_bps":p}));
    }
    if legs.is_empty() {
        return None;
    }
    Some((
        got,
        json!({"type":"indicative_price","rfq_legs":named,"legs":legs,"minimum_raw":floor.to_string(),
        "api_fees":book["fees"],"issues":book["issues"],"signed":false}),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn configured_origin_and_credential_scope_are_distinct() {
        let origin = "https://api.0x.org";
        let (u, auth) =
            endpoint("https://api.0x.org/", origin, "swap/allowance-holder/price").unwrap();
        assert!(auth);
        assert_eq!(u.as_str(), "https://api.0x.org/swap/allowance-holder/price");
        let (u, auth) = endpoint(
            "https://other.invalid/staging",
            origin,
            "swap/allowance-holder/price",
        )
        .unwrap();
        assert!(!auth);
        assert_eq!(u.path(), "/staging/swap/allowance-holder/price");
        for other in ["https://api.0x.org.evil.invalid", "https://api.0x.org:444"] {
            assert!(!endpoint(other, origin, "price").unwrap().1);
        }
        for invalid in [
            "http://api.0x.org",
            "https://forged@api.0x.org",
            "https://api.0x.org?key=secret",
            "https://api.0x.org/#fragment",
        ] {
            assert!(endpoint(invalid, origin, "price").is_none());
        }
    }
    #[test]
    fn mixed_aggregator_shows_maker_per_leg() {
        let a = Address::with_last_byte(1);
        let b = Address::with_last_byte(2);
        let mut response = json!({"liquidityAvailable":true,"sellToken":a,"buyToken":b,"sellAmount":"1000",
            "buyAmount":"990","minBuyAmount":"980","route":{"fills":[
             {"from":a,"to":b,"proportionBps":"3000","source":"0x_RFQ"},
             {"from":a,"to":b,"proportionBps":"7000","source":"Uniswap_V4"}]}});
        let (n, details) = zeroex(&response, a, b, U256::from(1000)).unwrap();
        assert_eq!(n, U256::from(990));
        assert_eq!(details["rfq_legs"], 1);
        assert_eq!(details["legs"].as_array().unwrap().len(), 2);
        assert!(zeroex(&response, b, a, U256::from(1000)).is_none());
        response["minBuyAmount"] = json!("1001");
        assert!(zeroex(&response, a, b, U256::from(1000)).is_none());
    }
}
