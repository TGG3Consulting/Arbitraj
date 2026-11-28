//! Ручная проверка и симуляция Hashflow Taker API v3 по запросу оператора.
//! https://docs.hashflow.com/hashflow/taker/getting-started-api-v3
pub(super) mod availability;
mod dispatch;
mod execute;
pub(super) mod feed;
mod market;
pub(super) mod routing;
pub(in crate::venues::bsc_dex) use execute::{ReceiptProof, failed, record};
pub(in crate::venues::bsc_dex) use dispatch::{marker,reserve,native_send};
#[cfg(test)]
pub(super) mod recovery_tests;
pub(super) mod model;
mod sim;
mod router_identity;
#[cfg(test)]
mod tests;
mod wire;

use super::{ctx::Ctx, node::chain_id};
use model::{Preview, Request};
use crate::gateway::{
    errors::{ErrorCode, GatewayResult},
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, U256, keccak256};
use chrono::Utc;
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeMap;

#[derive(Deserialize)]
struct Deployments {
    routers: BTreeMap<u64, Address>,
    router_runtime: BTreeMap<u64, alloy_primitives::B256>,
}

/// Общий опубликованный реестр адресов и отдельно проверенных runtime по сетям.
fn deployments() -> Result<&'static Deployments, String> {
    static DATA: std::sync::LazyLock<Option<Deployments>> = std::sync::LazyLock::new(||
        serde_json::from_str(include_str!("../../../config/hashflow_routers.json")).ok());
    DATA.as_ref().ok_or_else(|| "Повреждён реестр роутеров Hashflow".into())
}
/// Каноническая цель прямой RFQT из официального реестра.
fn router(chain: u64) -> Result<Address, String> {
    let d = deployments()?;
    d.routers
        .get(&chain)
        .copied()
        .ok_or_else(|| "Нет размещения Hashflow в этой сети".into())
}

/// Multi-wallet sources share levels. Individual API credentials belong to exactly one public address.
fn api_wallet(ctx:&Ctx,source:&str)->Result<Option<Address>,String>{
    if source.is_empty() || source.trim()!=source{return Err("Укажите идентификатор источника Hashflow без пробелов".into());}
    if source!="api"{return Ok(None);}
    ctx.addr("hashflow.api_wallet").ok().filter(|a|!a.is_zero()).map(Some)
        .ok_or_else(||"Для индивидуального ключа укажите публичный адрес в hashflow.api_wallet".into())
}
fn allowed(wallet:Option<Address>,trader:Address)->bool{wallet.is_none_or(|w|w==trader)}
pub(super) fn enabled(ctx: &Ctx) -> bool {
    ctx.flag("hashflow.trading").unwrap_or(false)
        && ctx.vault.has_secret("hashflow.taker_key")
        && ctx
            .text("hashflow.source")
            .is_ok_and(|s| api_wallet(ctx,&s).is_ok())
}

pub(super) async fn connection(ctx: &Ctx) -> Option<Result<String, String>> {
    let source = match ctx.text("hashflow.source") {
        Ok(s) if !s.trim().is_empty() => s,
        Ok(_) => return None,
        Err(e) if e.code == ErrorCode::NotConfigured => return None,
        Err(e) => return Some(Err(e.message)),
    };
    if !ctx.vault.has_secret("hashflow.taker_key") {
        return Some(Err(
            "Нет ключа Hashflow в разблокированном хранилище секретов".into(),
        ));
    }
    let chain = ctx.chain_id()?;
    if router(chain).is_err() {
        return None;
    }
    Some(
        market::makers(ctx, &source, chain, match api_wallet(ctx,&source){Ok(w)=>w,Err(e)=>return Some(Err(e))})
            .await
            .map(|m| format!("chainId {chain}: доступно маркетмейкеров {}", m.len())),
    )
}

/// Десятичный вывод сырых единиц без округления либо потери значимых цифр.
fn units(raw: U256, dp: u8) -> String {
    let mut s = raw.to_string();
    if dp == 0 {
        return s;
    }
    let dp = usize::from(dp);
    if s.len() <= dp {
        s = format!("{:0>width$}", s, width = dp + 1);
    }
    s.insert(s.len() - dp, '.');
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Ровно одна форма ввода. Decimal для уровней должен точно представлять весь введённый объём.
fn amount(request: &Request, dp: u8) -> Result<(U256, Decimal), String> {
    let raw = match (&request.amount, &request.amount_tokens) {
        (Some(s), None) => wire::raw(s)?,
        (None, Some(s)) => {
            let d = Decimal::from_str_exact(s).map_err(|_| "Неверная десятичная сумма токенов")?;
            if d.normalize().scale() > u32::from(dp) {
                return Err("Сумма содержит долю минимальной единицы токена".into());
            }
            super::num::to_units(d, dp).ok_or("Сумма не представима в единицах токена")?
        }
        _ => return Err("Укажите ровно один из amount и amount_tokens".into()),
    };
    if raw.is_zero() {
        return Err("Объём RFQ должен быть положительным".into());
    }
    let decimal = Decimal::from_str_exact(&units(raw, dp))
        .map_err(|_| "Точность объёма выходит за пределы расчёта уровней")?;
    Ok((raw, decimal))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Response {
    status: String,
    rfq_id: Option<String>,
    quotes: Option<Vec<wire::Firm>>,
}

/// Опрос подписанной котировки и read-only симуляция. Не сохраняет подпись и не отправляет транзакции.
pub(super) async fn preview(ctx: &Ctx, request: Request) -> GatewayResult<Preview> {
    let id = chain_id::chain_id(ctx)?;
    let source = ctx.text("hashflow.source")?;
    api_wallet(ctx,&source).map_err(|e|ctx.err(ErrorCode::NotConfigured,e))?;
    check(ctx, id, &source, request, false, None)
        .await
        .map(|q| q.preview)
        .map_err(|e| ctx.err(ErrorCode::BadResponse, e))
}

#[derive(Clone)]
struct Prepared {
    source: String,
    fallback_fee: Option<Decimal>,
    preview: Preview,
    firm: wire::Firm,
    request: Request,
    input: U256,
    output: U256,
    value: U256,
    di: u8,
    dout: u8,
    block: u64,
    gas: u64,
}

async fn check(
    ctx: &Ctx,
    id: u64,
    source: &str,
    r: Request,
    exact_output: bool,
    known: Option<(Vec<String>, Decimal)>,
) -> Result<Prepared, String> {
    if r.trader.is_zero() || r.input == r.output {
        return Err("Проверьте адрес трейдера и разные токены обмена".into());
    }
    if !ctx.vault.has_secret("hashflow.taker_key") {
        return Err("Нет ключа Hashflow в разблокированном хранилище секретов".into());
    }
    let wallet=api_wallet(ctx,source)?;
    if !allowed(wallet,r.trader){return Err("Hashflow: кошелёк поручения отличается от адреса индивидуального ключа".into());}
    let target = router(id)?;
    let h = sim::header(ctx, id).await?;
    let (di, do_) = tokio::try_join!(
        sim::decimals(ctx, r.input, h.hash),
        sim::decimals(ctx, r.output, h.hash)
    )?;
    let (desired, human) = amount(&r, if exact_output { do_ } else { di })?;
    let (makers, indicative) = match known {
        Some(x) => x,
        None => market::book(ctx, source, id, Some(r.input),wallet)
            .await?
            .at(&r, human, exact_output)?,
    };
    let amount_key = if exact_output {
        "quoteTokenAmount"
    } else {
        "baseTokenAmount"
    };
    let chain = json!({"chainType":"evm","chainId":id});
    let body = json!({"source":source,"baseChain":chain,"quoteChain":chain,"calldata":true,
        "rfqs":[{"baseToken":r.input,"quoteToken":r.output,(amount_key):desired.to_string(),"trader":r.trader,
            "effectiveTrader":r.trader,"marketMakers":makers,"options":{"doNotRetryWithOtherMakers":true}}]});
    let resp: Response = market::read(
        ctx,
        ctx.http
            .post("https://api.hashflow.com/taker/v3/rfq")
            .json(&body),
    )
    .await?;
    let rfq_id = resp
        .rfq_id
        .filter(|s| !s.is_empty())
        .ok_or("Hashflow не вернул RFQ ID")?;
    if resp.status != "success" {
        return Err("Маркетмейкер не предоставил подписанную котировку".into());
    }
    let quotes = resp.quotes.unwrap_or_default();
    let Ok([f]): Result<[_; 1], _> = quotes.try_into() else {
        return Err("Ожидалась одна котировка на одну заявку".into());
    };
    let now = || Utc::now().timestamp().max(0) as u64;
    let qty = if exact_output {
        wire::tradeRFQTCall::abi_decode_validate(&f.calldata)
            .map_err(|_| "Неверный ABI tradeRFQT")?
            .quote
            .effectiveBaseTokenAmount
    } else {
        desired
    };
    let (out, value) = wire::verify(&f, id, target, &r, qty, now().max(h.timestamp.to::<u64>()))?;
    if exact_output && out != desired {
        return Err("Подписанный выход расходится с размером сделки".into());
    }
    let gas = sim::run(ctx, h.hash, &r, &f, qty, out, value).await?;
    if now() >= f.quote_data.quote_expiry {
        return Err("Срок RFQ истёк во время симуляции".into());
    }
    let preview = Preview {
        chain_id: id,
        trader: r.trader,
        pool: f.quote_data.pool,
        rfq_id,
        txid: format!("{:#x}", f.quote_data.txid),
        amount_in: qty.to_string(),
        amount_out: out.to_string(),
        amount_out_tokens: units(out, do_),
        indicative_out: indicative.normalize().to_string(),
        target,
        selector: format!(
            "0x{}",
            alloy_primitives::hex::encode(wire::tradeRFQTCall::SELECTOR)
        ),
        calldata_hash: format!("{:#x}", keccak256(&f.calldata)),
        value: value.to_string(),
        expires: f.quote_data.quote_expiry,
        block_hash: format!("{:#x}", h.hash),
        gas_used: gas.to_string(),
        maker_count: makers.len(),
    };
    Ok(Prepared {
        source: source.to_owned(),
        fallback_fee: None,
        preview,
        firm: f,
        request: r,
        input: qty,
        output: out,
        value,
        di,
        dout: do_,
        block: h.number.to::<u64>(),
        gas: gas.to::<u64>(),
    })
}
