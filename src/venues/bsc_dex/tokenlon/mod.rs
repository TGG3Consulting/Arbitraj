//! Tokenlon PMMV5: персональная котировка по адресу кошелька и двум контрактам Ethereum.
mod client;
mod profile;
pub(crate) use profile::configured as wallet_pairs;
pub(super) mod roles;
pub(super) mod private_node;
pub(super) mod routing;
pub(super) mod book;
pub(super) mod settlement;
pub(super) mod recovery;
mod assets;
mod gas;
mod sim;
pub(super) mod receipt;
pub(super) mod wire;

use super::{num, parts::Parts, wallet};
use crate::{
    core::types::Side,
    gateway::{errors::{ErrorCode, GatewayResult}, source_quote::{Fee, Preview, Request}},
};
use alloy_primitives::U256;
use rust_decimal::Decimal;
use serde_json::json;

pub(super) const LABEL: &str = "Tokenlon PMMV5";
/// Сессия официального PMMV5 использует адреса Ethereum. В теме STOMP сеть не передаётся.
pub(crate) fn publisher_network(chain: Option<u64>) -> bool {
    chain == Some(crate::settings::evm_profiles::ETHEREUM_MAINNET)
}
pub(super) fn enabled(c:&super::ctx::Ctx)->bool {
    publisher_network(c.chain_id()) && c.flag("tokenlon.trading").unwrap_or(false)
}
pub(super) fn timely(c:&super::ctx::Ctx,expiry:u64)->bool {
    c.int("tokenlon.min_validity_ms").is_ok_and(|n|n>0 && super::hashflow::routing::expiry_ok(expiry,n))
}
fn quantity(field: &serde_json::Value) -> Option<U256> {
    if let Some(s) = field.as_str() {
        return (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .then(|| U256::from_str_radix(s, 10).ok()).flatten();
    }
    field.as_u64().map(U256::from)
}

pub(super) async fn preview(p: &Parts, req: Request) -> GatewayResult<Preview> {
    let ctx = &p.ctx;
    let bad = |s| ctx.err(ErrorCode::NotApplicable, s);
    let wallet_id = ctx.bound()?;
    if req.market.is_some() || req.builder.is_some() || req.amount_tokens.is_some()
        || req.firm.as_ref().is_some_and(|f| f.allow_trade || f.wallet != wallet_id || f.fee_tenths_bps.is_some()) {
        return Err(bad("Проверка цены Tokenlon требует выбранного кошелька и одной точной суммы в raw"));
    }
    let side = match (&req.amount_raw, &req.buy_amount_raw) {
        (Some(_), None) => Side::Sell,
        (None, Some(_)) => Side::Buy,
        _ => return Err(bad("Для SELL задайте amount_raw, для BUY — buy_amount_raw")),
    };
    let (base, quote) = match side {Side::Buy => (req.output,req.input), Side::Sell => (req.input,req.output)};
    let market = profile::markets(ctx).and_then(|v| v.into_iter()
        .find(|m| m.base_token == base && m.quote_token == quote))
        .ok_or_else(|| ctx.err(ErrorCode::NotConfigured,"Пара адресов не найдена в tokenlon.pairs площадки Ethereum"))?;
    let roles=roles::FrozenRoles::read(ctx).ok_or_else(||bad("Для Tokenlon не указан профиль контрактов"))?;
    let key = wallet::signer::signer(ctx)?;
    let payer = key.address();
    if req.trader != Some(payer) {
        return Err(bad("Публичный адрес трейдера должен совпадать с выбранным кошельком Tokenlon"));
    }
    let (base_dec, quote_dec) = tokio::try_join!(p.tokens.decimals(ctx,base),p.tokens.decimals(ctx,quote))?;
    let amount_raw = num::exact_amount(req.amount_raw.as_deref().or(req.buy_amount_raw.as_deref()), None, base_dec)
        .ok_or_else(|| bad("Нужна положительная целая сумма base в raw"))?;
    let amount = num::to_decimal(amount_raw,base_dec)
        .filter(|v| num::to_units(*v, base_dec) == Some(amount_raw))
        .ok_or_else(|| bad("Сумма base превышает точность денежного расчёта"))?;
    let query = client::Query {base:market.base, quote:market.quote, side, amount, base_dec,
        input:req.input, output:req.output};
    let (spent, received, fee, expires, mut details) = if let Some(f) = req.firm {
        let min = num::exact_amount(Some(&f.minimum_raw),None,0).ok_or_else(|| bad("Некорректный минимум поступления"))?;
        let maximum = f.maximum_raw.as_deref().map(|v| num::exact_amount(Some(v),None,0)
            .ok_or_else(|| bad("Некорректный предел расхода"))).transpose()?;
        let issued = client::firm(ctx,&key,&query).await?;
        let spent = quantity(&issued.raw_order["takerAssetAmount"])
            .filter(|spent| maximum.is_none_or(|cap| *spent <= cap)).ok_or_else(|| bad("Расход firm Tokenlon превышает предел или отсутствует"))?;
        let terms = wire::Terms {roles,payer, recipient:payer, input:req.input, output:req.output, spent, minimum:min};
        // Без подписи taker, отправки и регистрации торгового поручения.
        let now = u64::try_from(chrono::Utc::now().timestamp()).map_err(|_| bad("Некорректное время запроса"))?;
        let proof = wire::prepare(&issued.raw_order,&terms,U256::ZERO,now)
            .ok_or_else(|| bad("Firm Tokenlon не прошёл проверку PMMV5, срока, условий кошелька или минимума"))?.proof;
        (spent,proof.net,Some(Fee{asset:req.output,amount_raw:(proof.gross-proof.net).to_string()}),
            Some(proof.expiry),json!({"native_input":req.input==roles.weth.address,"native_output":req.output==roles.weth.address,"stage":"maker_order","maker":proof.maker,"orderHash":proof.order_hash,"moneySimulated":false}))
    } else {
        let rate = client::rate(ctx,&key,&query).await?;
        let other = indicative(side, amount, rate, quote_dec)
            .ok_or_else(|| bad("Ставка Tokenlon не даёт положительной суммы в пределах точности"))?;
        let (spent,received) = match side {Side::Sell=>(amount_raw,other),Side::Buy=>(other,amount_raw)};
        (spent,received,None,None,json!({"native_input":req.input==roles.weth.address,"native_output":req.output==roles.weth.address,"stage":"indicative","rate":rate,"moneySimulated":false}))
    };
    details["localSimulationConfigured"]=json!(private_node::configured(ctx));
    details["automaticTradingEnabled"]=json!(enabled(ctx));
    Ok(Preview {source:"tokenlon".into(), input:req.input, output:req.output, spent_raw:spent.to_string(),
        received_raw:received.to_string(), unspent_raw:"0".into(), fee, expires,
        block_hash:String::new(), block:None, details})
}

fn indicative(side: Side, amount: Decimal, rate: Decimal, quote_dec: u8) -> Option<U256> {
    if rate <= Decimal::ZERO {return None;}
    let value = match side { Side::Sell => amount.checked_mul(rate)?, Side::Buy => amount.checked_div(rate)? };
    let raw = num::to_units(value,quote_dec)?;
    let rounded = if side == Side::Buy && num::to_decimal(raw, quote_dec)? < value {raw.checked_add(U256::from(1))?} else {raw};
    (!rounded.is_zero()).then_some(rounded)
}
