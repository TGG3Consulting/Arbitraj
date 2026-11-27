//! Ручная котировка Fynd Hosted. Подпись, разрешение на токены и ордер здесь не создаются.
use super::{ctx::Ctx, quote::white_state};
use crate::gateway::{
    errors::{ErrorCode, GatewayResult},
    http,
    source_quote::{Preview, Request},
};
use alloy_primitives::{Address, B256, U256};
use reqwest::{Client, header::{AUTHORIZATION, HeaderValue}};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
struct KnownPool {
    chain: u64,
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

fn source_legs(chain: u64, legs: &[Value]) -> Option<usize> {
    let pools: Vec<KnownPool> = serde_json::from_str(include_str!("../../../config/fynd_pools.json")).ok()?;
    Some(legs.iter().filter(|s| {
        let id = s["component_id"].as_str().and_then(|t| t.parse::<B256>().ok());
        let input = s["token_in"].as_str().and_then(|t| t.parse::<Address>().ok());
        let output = s["token_out"].as_str().and_then(|t| t.parse::<Address>().ok());
        pools.iter().any(|p| p.chain == chain && Some(p.id) == id &&
            ((input,output)==(Some(p.tokens[0]),Some(p.tokens[1])) ||
             (input,output)==(Some(p.tokens[1]),Some(p.tokens[0]))))
    }).count())
}

pub(super) async fn preview(ctx: &Ctx, req: Request) -> GatewayResult<Preview> {
    let bad = |why: &str| ctx.err(ErrorCode::BadResponse, why);
    let (slug,chain) = match ctx.chain_id() {
        Some(8453) => ("base",8453),
        Some(1) => ("ethereum",1),
        _ => return Err(ctx.err(ErrorCode::NotApplicable, "Fynd Hosted обслуживает Base и Ethereum")),
    };
    let who = req.trader.filter(|s| !s.is_zero()).ok_or_else(|| bad("Укажите публичный адрес плательщика Fynd"))?;
    let amount = req.amount_raw.as_deref().filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|s| U256::from_str_radix(s,10).ok()).filter(|n| !n.is_zero())
        .ok_or_else(|| bad("Укажите положительный amount_raw в минимальных единицах"))?;
    if req.input == req.output || req.market.is_some() || req.builder.is_some() || req.amount_tokens.is_some() {
        return Err(bad("Для ручной цены Fynd нужны две разные валюты, amount_raw и адрес плательщика"));
    }
    let key = ctx.vault.get("fynd.api_key").map_err(|_| bad("Откройте хранилище и задайте fynd.api_key"))?;
    if key.trim().is_empty() { return Err(bad("Не заполнен fynd.api_key")); }
    let mut header=HeaderValue::from_str(key.as_str()).map_err(|_| bad("Некорректное значение fynd.api_key"))?;
    header.set_sensitive(true);
    let root=format!("https://fynd-api.propellerheads.xyz/v1/{slug}");
    let client=Client::builder().redirect(reqwest::redirect::Policy::none()).retry(reqwest::retry::never())
        .build().map_err(|_| bad("Не создан HTTP клиент Fynd"))?;
    let limits=ctx.limits()?;
    let read=|r:reqwest::RequestBuilder| {
        let request=r.header(AUTHORIZATION,header.clone());
        async move {
            let (status,body)=http::exchange(request,limits).await.map_err(|_| bad("Нет полного ответа Fynd"))?;
            if !status.is_success() {
                return Err(bad(&format!("Fynd HTTP {} для выбранной сети",status.as_u16())));
            }
            serde_json::from_str::<Value>(&body).map_err(|_| bad("Fynd вернул некорректный JSON"))
        }
    };
    let info=read(client.get(format!("{root}/info"))).await?;
    if info["chain_id"].as_u64()!=Some(chain) { return Err(bad("Fynd сообщил другую сеть")); }
    let result=read(client.post(format!("{root}/quote")).json(&json!({"orders":[{
        "token_in":req.input,"token_out":req.output,"sender":who,"receiver":who,"side":"sell","amount":amount.to_string()
    }]}))).await?;
    let orders=result["orders"].as_array().ok_or_else(||bad("Нет списка котировок Fynd"))?;
    let [q]=orders.as_slice() else { return Err(bad("Ожидалась одна котировка Fynd")); };
    if q["status"]!="success" {
        let reason=match q["status"].as_str() {
            Some("no_route_found")=>"маршрут не найден",Some("insufficient_liquidity")=>"не хватает ликвидности",
            Some("timeout")=>"время поиска истекло",Some("not_ready")=>"источник ещё не готов",
            Some("price_check_failed")=>"проверка цены отклонила результат",Some("encoding_failed")=>"не удалось собрать маршрут",
            _=>"неизвестный статус",
        };
        return Err(ctx.err(ErrorCode::NoRoute,format!("Fynd: {reason}")));
    }
    let out=raw(&q["amount_out"]).filter(|n| !n.is_zero()).ok_or_else(||bad("Нет положительного выхода Fynd"))?;
    if raw(&q["amount_in"])!=Some(amount) {return Err(bad("Объём котировки Fynd отличается от запроса"));}
    let block=q["block"]["number"].as_u64().ok_or_else(||bad("Нет номера блока Fynd"))?;
    let h=white_state::head(ctx,&format!("0x{block:x}")).await?;
    if q["block"]["hash"].as_str().and_then(|v|v.parse::<B256>().ok())!=Some(h.hash)
        || h.number.to::<u64>()!=block || q["block"]["timestamp"].as_u64()!=Some(h.timestamp.to::<u64>()) {
        return Err(bad("Блок Fynd отличается от канонического блока узла"));
    }
    let legs=q["route"]["swaps"].as_array().filter(|v| !v.is_empty()).ok_or_else(||bad("Нет состава маршрута Fynd"))?;
    let own=source_legs(chain,legs).ok_or_else(||bad("Ошибка реестра собственных пулов Fynd"))?;
    Ok(Preview{source:"fynd".into(),input:req.input,output:req.output,spent_raw:amount.to_string(),received_raw:out.to_string(),
        unspent_raw:"0".into(),fee:None,block_hash:h.hash.to_string(),block:Some(block),expires:None,
        details:json!({"indicative":true,"swaps":legs.len(),"recognized_fynd_swaps":own,
            "amount_out_net_gas_raw":raw(&q["amount_out_net_gas"]).map(|v|v.to_string())})})
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::GzDecoder;

    #[test]
    fn both_directions_match_only_canonical_pool_and_currency_on_the_right_chain() {
        for (chain, bytes) in [
            (8453, include_bytes!("../../../tests/data/first_run_dex_2026-11-26/fynd_base_markets.json.gz").as_slice()),
            (1, include_bytes!("../../../tests/data/first_run_dex_2026-11-26/fynd_ethereum_markets.json.gz").as_slice()),
        ] {
            let inventory: Value = serde_json::from_reader(GzDecoder::new(bytes)).unwrap();
            for m in inventory["markets"].as_array().unwrap() {
                for (a,b) in [(0,1),(1,0)] {
                    let swap = json!({"component_id":m["id"],"token_in":m["tokens"][a],"token_out":m["tokens"][b]});
                    let unknown = json!({"protocol":"Fynd AMM","component_id":B256::ZERO,"token_in":m["tokens"][a],"token_out":m["tokens"][b]});
                    assert_eq!(source_legs(chain,&[swap.clone(),unknown]),Some(1));
                    assert_eq!(source_legs(56,&[swap]),Some(0));
                    let wrong_pair=json!({"component_id":m["id"],"token_in":m["tokens"][a],"token_out":m["tokens"][a]});
                    assert_eq!(source_legs(chain,&[wrong_pair]),Some(0));
                }
            }
        }
    }
}
