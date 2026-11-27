//! Формирует и сохраняет результат подтверждённого обмена после сверки квитанции.
//! При повторном чтении сверяет обе суммы сделки и расходы с исходным включением в блок.
//! Основание учёта — docs/DECISIONS.md, решения 222 и 413. В contracts/PropSlotVenueAdapter.sol
//! executeAtSlot передаёт точный вход и минимум выплаты в PropVenueAdapter.execute.
use super::{Marker, Parts, intent, resolve, unknown};
use crate::{core::types::{ChainTx, Fee, Side, TradeRequest, TradeResult, TradeStatus},
    gateway::{errors::GatewayResult, pending::PendingRef},
    venues::bsc_dex::{fermi::slot::Proof, num}};
use alloy::primitives::U256;
use chrono::Utc;
use rust_decimal::Decimal;

/// Для покупки PropAMM зачисляем весь подтверждённый выход при точном совпадении подписанного
/// минимума с заявкой. У Fermi и при продаже требуется точное количество.
pub(in crate::venues::bsc_dex::fermi) fn amounts(
    o: &Proof, req: &TradeRequest, (coin_dec, cash_dec): (u8, u8), (spent, received): (U256, U256),
) -> Option<(Decimal, Decimal)> {
    let (coins, cash) = if req.side == Side::Buy { (received, spent) } else { (spent, received) };
    let coins = num::to_decimal(coins, coin_dec)?;
    let gross = num::to_decimal(cash, cash_dec)?;
    let matched = if let (Some(q), Side::Buy) = (&o.prop, req.side) {
        o.amount.is_negative()
            && req.asset.contract == Some(o.output)
            && q.minimum == o.amount.unsigned_abs()
            && num::to_units(req.qty, coin_dec) == Some(q.minimum)
            && num::to_decimal(q.minimum, coin_dec) == Some(req.qty)
            && received >= q.minimum
    } else {
        coins == req.qty
    };
    matched.then_some((coins, gross))
}

pub(super) async fn finish(
    p: &Parts, req: TradeRequest, marker: &PendingRef, parse: &Marker, entry: &intent::Entry,
    chain: ChainTx, (spent, received): (U256, U256),
) -> GatewayResult<Option<TradeResult>> {
    let o = &parse.order;
    let (hash, nonce) = (chain.hash, chain.nonce);
    let db = p.ctx.journal.db();
    let unknown = || unknown(p, marker);
    // Решение 222: зачисляем только фактическую выплату сделки. Подтверждение исходной
    // подписи, SlotFilled и чистых переводов владельца предшествует записи денег.
    let Some((coins, gross)) = amounts(o, &req, parse.decs, (spent, received)) else {
        if o.prop.is_some() {
            return Err(p.ctx.err(crate::gateway::errors::ErrorCode::OutcomeUnknown,
                format!("PropAMM: подтверждено списание {spent} атомов {} и поступление {received} атомов {}; требуется ручная сверка исходной заявки", o.input, o.output))
                .with_tx(chain).pending(marker.clone()));
        }
        return Err(unknown());
    };
    let fee = chain.gas_bnb.ok_or_else(unknown)?;
    if let Some(intent::Outcome::Order(out)) = &entry.result {
        if out.trade.filled_qty != coins
            || out.gross_quote != gross
            || out.trade.venue_ref != hash.to_string()
            || out.trade.chain.as_ref().is_none_or(|c| {
                (c.hash, c.nonce, c.block, c.gas_used, c.gas_bnb)
                    != (
                        chain.hash,
                        chain.nonce,
                        chain.block,
                        chain.gas_used,
                        chain.gas_bnb,
                    )
            })
        {
            return Err(unknown());
        }
        resolve::release_if_ours(p, hash, Some(nonce), false).await;
        intent::note_gross(Some(gross));
        return Ok(Some(out.trade.clone()));
    }
    let spot = spot(o, req, chain, coins, gross, fee).ok_or_else(unknown)?;
    let trade = spot.trade.clone();
    intent::confirm(
        db,
        &parse.key,
        entry,
        intent::Outcome::Order(Box::new(spot)),
    )
    .await
    .map_err(|_| unknown())?;
    resolve::release_if_ours(p, hash, Some(nonce), false).await;
    intent::note_gross(Some(gross));
    Ok(Some(trade))
}

/// Возвращает денежный результат, сохраняемый перед дальнейшим движением монет.
/// Без вычислимой цены результат остаётся на ручной сверке.
pub(in crate::venues::bsc_dex::fermi) fn spot(
    o: &Proof, req: TradeRequest, chain: ChainTx, coins: Decimal, gross: Decimal, fee: Decimal,
) -> Option<intent::SpotFill> {
    let hash = chain.hash;
    let trade = TradeResult {
        status: TradeStatus::Filled,
        filled_qty: coins,
        avg_price: gross.checked_div(coins)?,
        fees: Fee::single(fee, "ETH"),
        venue_ref: hash.to_string(),
        chain: Some(chain),
        finished_at: Utc::now(),
        executor: Some(match &o.prop {
            Some(q) => format!("PropAMM {} ({})", q.terms.venue, o.builder),
            None => format!("FermiSwap ({})", o.builder),
        }),
        request: req,
    };
    Some(intent::SpotFill {
        trade,
        gross_quote: gross,
    })
}
