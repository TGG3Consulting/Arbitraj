//! Устойчивое поручение Fermi. Подпись относится к одному плательщику и слоту; восстановление читает только сохранённый хеш.
//! Вход из торгового или расчётного шага, зависящий от идентичности счёта, билета и кадра сборщика.
#[path = "execute_submit.rs"]
mod submission;
use submission::submit;
use super::super::{num, parts::Parts};
use super::{routing::Prepared, slot::Proof};
use crate::{
    core::types::{RouteMode, Side, TradeRequest, TradeResult},
    gateway::{
        errors::{ErrorCode, GatewayResult},
        settlement_intent as intent,
    },
};
use alloy::primitives::{Address, B256, keccak256};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Serialize, Deserialize)]
pub(super) struct Marker {
    pub order: Proof,
    pub decs: (u8, u8),
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outer_account: Option<String>,
}
pub(super) fn identity(p: &Parts, r: &TradeRequest) -> GatewayResult<String> {
    if r.deal_id.is_empty() || r.asset.contract.is_none() || r.venue != p.ctx.venue() {
        return Err(p.ctx.err(
            ErrorCode::NotApplicable,
            "Fermi: необходимы исходные ID сделки, монеты и площадки",
        ));
    }
    let account = p.ctx.account.as_deref().ok_or_else(|| {
        p.ctx
            .err(ErrorCode::NotApplicable, "Fermi: требуется счёт сделки")
    })?;
    Ok(format!(
        "fermi/{}",
        keccak256(
            json!([
                1,
                p.ctx.place.id,
                account,
                r.asset.contract,
                r.side,
                r.deal_id
            ])
            .to_string()
            .as_bytes()
        )
    ))
}
pub(super) fn same_request(r: &TradeRequest, t: &TradeRequest) -> bool {
    r.deal_id == t.deal_id
        && r.venue == t.venue
        && r.asset.contract == t.asset.contract
        && r.side == t.side
        && r.qty == t.qty
        && r.asset.chain == t.asset.chain
}
fn timely(p: &Parts, q: &Prepared) -> bool {
    super::slot_config::timely(p, &q.book)
}
fn check(p: &Parts, r: &TradeRequest, q: &Prepared, who: Address) -> GatewayResult<()> {
    let bad = || {
        p.ctx.err(
            ErrorCode::QuoteMismatch,
            "Fermi: сумма, предел цены, счёт или кадр изменились",
        )
    };
    let buy = r.side == Side::Buy;
    let o = &q.proof;
    let coin = r.asset.contract.ok_or_else(bad)?;
    let cash = q.book.config.cash;
    let (i, out) = if buy { (cash, coin) } else { (coin, cash) };
    let fixed = num::to_decimal(o.amount.unsigned_abs(), q.decs.0);
    let gross =
        num::to_decimal(if buy { q.spent } else { q.received }, q.decs.1).ok_or_else(bad)?;
    let limit = r.qty.checked_mul(r.worst_price).ok_or_else(bad)?;
    if !timely(p, q)
        || who != o.payer
        || (i, out) != (o.input, o.output)
        || fixed != Some(r.qty)
        || r.qty <= Decimal::ZERO
        || buy != o.amount.is_negative()
        || (buy && gross > limit)
        || (!buy && gross < limit)
        || r.remeasure
            .as_ref()
            .is_some_and(|v| !v.any_executor || (!buy && gross < v.floor))
    {
        return Err(bad());
    }
    Ok(())
}
pub(in crate::venues::bsc_dex) async fn trade(
    p: &Parts,
    req: TradeRequest,
    id: B256,
) -> GatewayResult<TradeResult> {
    if req.route.as_ref().map(|r| r.mode) != Some(RouteMode::Slot(id)) {
        return Err(p.ctx.err(
            ErrorCode::NotApplicable,
            "Для слота нужен исходный денежный шаг",
        ));
    }
    submit(p, req, id, None).await
}
pub(in crate::venues::bsc_dex) async fn trade_settlement(
    p: &Parts,
    req: TradeRequest,
    id: B256,
    cash: Address,
) -> GatewayResult<TradeResult> {
    if req.route.as_ref().map(|r| r.mode) != Some(RouteMode::PropSettlementSlot(Some(id)))
        || cash != p.tokens.usdt(&p.ctx)?
        || !intent::active()
        || !crate::gateway::settlement_proofs::active()
    {
        return Err(p.ctx.err(
            ErrorCode::NotApplicable,
            "Нет исходного денежного поручения и привязки к его кассе",
        ));
    }
    submit(p, req, id, Some(cash)).await
}
