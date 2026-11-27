//! Ручная котировка Fynd Hosted. Подпись, разрешение на токены и ордер здесь не создаются.
#[path = "fynd_flow.rs"]
mod flow;
use super::{ctx::Ctx, quote::white_state};
use crate::gateway::{
    errors::{ErrorCode, GatewayResult},
    http,
    source_quote::{Preview, Request},
};
use alloy_primitives::{Address, B256, U256};
use reqwest::{
    Client,
    header::{AUTHORIZATION, HeaderValue},
};

use serde_json::{Value, json};

struct KnownPool {
    id: B256,
    tokens: [Address; 2],
}

fn raw(value: &Value) -> Option<U256> {
    let text = value.as_str()?;
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    U256::from_str_radix(text, 10).ok()
}

fn pool(row: &str) -> Option<KnownPool> {
    let mut parts = row.split('=');
    let id = parts.next()?.parse().ok()?;
    let tokens = [
        parts.next()?.parse::<Address>().ok()?,
        parts.next()?.parse::<Address>().ok()?,
    ];
    (tokens[0] < tokens[1] && parts.next().is_none()).then_some(KnownPool { id, tokens })
}
fn endpoint(base: &str, slug: &str, method: &str) -> Option<reqwest::Url> {
    let mut url = reqwest::Url::parse(base).ok()?;
    if url.scheme() != "https"
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || slug.is_empty()
        || !slug.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return None;
    }
    url.path_segments_mut()
        .ok()?
        .pop_if_empty()
        .push(slug)
        .push(method);
    Some(url)
}
fn source_legs(pools: &[KnownPool], legs: &[Value]) -> usize {
    legs.iter()
        .filter(|s| {
            let id = s["component_id"]
                .as_str()
                .and_then(|t| t.parse::<B256>().ok());
            let input = s["token_in"]
                .as_str()
                .and_then(|t| t.parse::<Address>().ok());
            let output = s["token_out"]
                .as_str()
                .and_then(|t| t.parse::<Address>().ok());
            pools.iter().any(|p| {
                Some(p.id) == id
                    && ((input, output) == (Some(p.tokens[0]), Some(p.tokens[1]))
                        || (input, output) == (Some(p.tokens[1]), Some(p.tokens[0])))
            })
        })
        .count()
}

pub(super) async fn preview(ctx: &Ctx, req: Request) -> GatewayResult<Preview> {
    let bad = |why: &str| ctx.err(ErrorCode::BadResponse, why);
    let chain = ctx
        .chain_id()
        .ok_or_else(|| bad("Не определена сеть Fynd"))?;
    let slug = ctx.text("fynd.network")?;
    let base = ctx.text("fynd.base_url")?;
    let info_url = endpoint(&base, &slug, "info")
        .ok_or_else(|| bad("Некорректный HTTPS адрес или имя сети Fynd"))?;
    let quote_url =
        endpoint(&base, &slug, "quote").ok_or_else(|| bad("Некорректный HTTPS адрес Fynd"))?;
    let pools = ctx
        .settings
        .list("fynd.pools", ctx.at())
        .unwrap_or_default()
        .iter()
        .map(|r| pool(r))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| bad("Строка fynd.pools должна содержать PoolId=token0=token1"))?;
    let who = req
        .trader
        .filter(|s| !s.is_zero())
        .ok_or_else(|| bad("Укажите публичный адрес плательщика Fynd"))?;
    let amount = req
        .amount_raw
        .as_deref()
        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|s| U256::from_str_radix(s, 10).ok())
        .filter(|n| !n.is_zero())
        .ok_or_else(|| bad("Укажите положительный amount_raw в минимальных единицах"))?;
    if req.input == req.output
        || req.market.is_some()
        || req.builder.is_some()
        || req.amount_tokens.is_some()
    {
        return Err(bad(
            "Для ручной цены Fynd нужны две разные валюты, amount_raw и адрес плательщика",
        ));
    }
    // Ключ Hosted может получить только официальный TLS origin. Свой сервер допускается без передачи ключа Hosted.
    let header = if info_url.origin().ascii_serialization() == "https://fynd-api.propellerheads.xyz"
    {
        let key = ctx
            .vault
            .get("fynd.api_key")
            .map_err(|_| bad("Откройте хранилище и задайте fynd.api_key"))?;
        if key.trim().is_empty() {
            return Err(bad("Не заполнен fynd.api_key"));
        }
        let mut h = HeaderValue::from_str(key.as_str())
            .map_err(|_| bad("Некорректное значение fynd.api_key"))?;
        h.set_sensitive(true);
        Some(h)
    } else {
        None
    };
    // Не переадресуем Authorization и не повторяем POST, даже если узел разорвал соединение.
    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .build()
        .map_err(|_| bad("Не создан HTTP клиент Fynd"))?;
    let limits = ctx.limits()?;
    let read = |r: reqwest::RequestBuilder| {
        let request = match &header {
            Some(h) => r.header(AUTHORIZATION, h.clone()),
            None => r,
        };
        async move {
            let (status, body) = http::exchange(request, limits)
                .await
                .map_err(|_| bad("Нет полного ответа Fynd"))?;
            if !status.is_success() {
                return Err(bad(&format!(
                    "Fynd HTTP {} для выбранной сети",
                    status.as_u16()
                )));
            }
            serde_json::from_str::<Value>(&body).map_err(|_| bad("Fynd вернул некорректный JSON"))
        }
    };
    let info = read(client.get(info_url)).await?;
    if info["chain_id"].as_u64() != Some(chain) {
        return Err(bad("Fynd сообщил другую сеть"));
    }
    let result=read(client.post(quote_url).json(&json!({"orders":[{
        "token_in":req.input,"token_out":req.output,"sender":who,"receiver":who,"side":"sell","amount":amount.to_string()
    }]}))).await?;
    let orders = result["orders"]
        .as_array()
        .ok_or_else(|| bad("Нет списка котировок Fynd"))?;
    let [q] = orders.as_slice() else {
        return Err(bad("Ожидалась одна котировка Fynd"));
    };
    if q["status"] != "success" {
        let reason = match q["status"].as_str() {
            Some("no_route_found") => "маршрут не найден",
            Some("insufficient_liquidity") => "не хватает ликвидности",
            Some("timeout") => "время поиска истекло",
            Some("not_ready") => "источник ещё не готов",
            Some("price_check_failed") => "проверка цены отклонила результат",
            Some("encoding_failed") => "не удалось собрать маршрут",
            _ => "неизвестный статус",
        };
        return Err(ctx.err(ErrorCode::NoRoute, format!("Fynd: {reason}")));
    }
    let out = raw(&q["amount_out"])
        .filter(|n| !n.is_zero())
        .ok_or_else(|| bad("Нет положительного выхода Fynd"))?;
    if raw(&q["amount_in"]) != Some(amount) {
        return Err(bad("Объём котировки Fynd отличается от запроса"));
    }
    let block = q["block"]["number"]
        .as_u64()
        .ok_or_else(|| bad("Нет номера блока Fynd"))?;
    let h = white_state::head(ctx, &format!("0x{block:x}")).await?;
    if q["block"]["hash"]
        .as_str()
        .and_then(|v| v.parse::<B256>().ok())
        != Some(h.hash)
        || h.number.to::<u64>() != block
        || q["block"]["timestamp"].as_u64() != Some(h.timestamp.to::<u64>())
    {
        return Err(bad("Блок Fynd отличается от канонического блока узла"));
    }
    let legs = q["route"]["swaps"]
        .as_array()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| bad("Нет состава маршрута Fynd"))?;
    flow::balanced(legs, req.input, req.output, amount, out)
        .ok_or_else(|| bad("Шаги Fynd не сводятся с валютами или суммами заявки"))?;
    let fees = if q["fee_breakdown"].is_null() {
        None
    } else {
        Some(
            flow::fees(&q["fee_breakdown"], out)
                .ok_or_else(|| bad("Не сходятся комиссии и минимум Fynd"))?,
        )
    };
    let latest = white_state::head(ctx, "latest").await?;
    let age = latest
        .number
        .to::<u64>()
        .checked_sub(block)
        .ok_or_else(|| bad("Котировка Fynd опережает узел"))?;
    let own = source_legs(&pools, legs);
    Ok(Preview {
        source: "fynd".into(),
        input: req.input,
        output: req.output,
        spent_raw: amount.to_string(),
        received_raw: out.to_string(),
        unspent_raw: "0".into(),
        fee: None,
        block_hash: h.hash.to_string(),
        block: Some(block),
        expires: None,
        details: json!({"indicative":true,"swaps":legs.len(),"recognized_fynd_swaps":own,"age_blocks":age,
            "router_fee_raw":fees.as_ref().map(|f|f.router.to_string()),"client_fee_raw":fees.as_ref().map(|f|f.client.to_string()),
            "minimum_received_raw":fees.as_ref().map(|f|f.minimum.to_string()),
            "amount_out_net_gas_raw":raw(&q["amount_out_net_gas"]).map(|v|v.to_string())}),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::GzDecoder;

    #[test]
    fn both_directions_match_only_canonical_pool_and_currency_on_the_right_chain() {
        for (chain, bytes) in [
            (
                8453,
                include_bytes!(
                    "../../../tests/data/first_run_dex_2026-11-26/fynd_base_markets.json.gz"
                )
                .as_slice(),
            ),
            (
                1,
                include_bytes!(
                    "../../../tests/data/first_run_dex_2026-11-26/fynd_ethereum_markets.json.gz"
                )
                .as_slice(),
            ),
        ] {
            let inventory: Value = serde_json::from_reader(GzDecoder::new(bytes)).unwrap();
            let config: toml::Value =
                toml::from_str(include_str!("../../../config/first_run.toml")).unwrap();
            let venue = if chain == 8453 { "base_dex" } else { "eth_dex" };
            let rec = config["section"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|s| s.get("rec")?.as_array())
                .flatten()
                .find(|r| {
                    r["name"].as_str() == Some("fynd.pools")
                        && r["prefill"]["id"].as_str() == Some(venue)
                })
                .unwrap();
            let pools: Vec<_> = rec["value"]
                .as_str()
                .unwrap()
                .split(',')
                .map(|s| pool(s).unwrap())
                .collect();
            assert_eq!(pools.len(), inventory["markets"].as_array().unwrap().len());
            for m in inventory["markets"].as_array().unwrap() {
                for (a, b) in [(0, 1), (1, 0)] {
                    let swap = json!({"component_id":m["id"],"token_in":m["tokens"][a],"token_out":m["tokens"][b]});
                    let unknown = json!({"protocol":"Fynd AMM","component_id":B256::ZERO,"token_in":m["tokens"][a],"token_out":m["tokens"][b]});
                    assert_eq!(source_legs(&pools, &[swap.clone(), unknown]), 1);
                    assert_eq!(source_legs(&[], &[swap]), 0);
                    let wrong_pair = json!({"component_id":m["id"],"token_in":m["tokens"][a],"token_out":m["tokens"][a]});
                    assert_eq!(source_legs(&pools, &[wrong_pair]), 0);
                }
            }
        }
    }
}
