//! Объёмы принятой заявки и стоимость релейной транзакции для кошелька.
use super::super::{exec, num, parts::Parts, send};
use super::{chain, ledger::Marker};
use crate::{
    core::types::{SendMethod, Side, TradeRequest, TradeResult, TradeStatus, TxOutcome},
    gateway::{
        errors::{ErrorCode, GatewayResult},
        settlement_intent as intent,
    },
};
use alloy::primitives::B256;
use chrono::Utc;
use rust_decimal::Decimal;
pub(super) fn filled(
    p: &Parts,
    r: TradeRequest,
    m: &Marker,
    t: chain::Mined,
) -> GatewayResult<intent::SpotFill> {
    let bad = || {
        p.ctx.err(
            ErrorCode::OutcomeUnknown,
            "Ryze: не удалось выразить суммы выплаты",
        )
    };
    // Возврат того же входного актива от выбранного пула сохранён в фактах отдельно. Чужие платежи не улучшают результат.
    let net_input = t
        .fill
        .input_to_router
        .checked_sub(t.fill.input_from_path)
        .ok_or_else(bad)?;
    let (amount, cash) = if r.side == Side::Buy {
        (t.fill.output_from_router, net_input)
    } else {
        (net_input, t.fill.output_from_router)
    };
    let coins = num::to_decimal(amount, m.decimals.0)
        .filter(|v| *v > Decimal::ZERO)
        .ok_or_else(bad)?;
    let gross = num::to_decimal(cash, m.decimals.1).ok_or_else(bad)?;
    let tracked = send::track::Tracked {
        outcome: TxOutcome::Included,
        receipt: Some(t.receipt),
        cancel: None,
        unresolved: None,
    };
    let mut tx = exec::chain_tx::chain_tx(
        t.hash,
        t.tx_nonce,
        vec!["Ryze Relayer".into()],
        SendMethod::Single,
        &tracked,
        None,
    );
    // gas_used относится к целой транзакции релея. Чистый расход ETH кошелька на её включение равен нулю.
    tx.gas_bnb = Some(Decimal::ZERO);
    Ok(intent::SpotFill {
        trade: TradeResult {
            status: TradeStatus::Filled,
            filled_qty: coins,
            avg_price: gross.checked_div(coins).ok_or_else(bad)?,
            fees: vec![],
            venue_ref: t.hash.to_string(),
            chain: Some(tx),
            finished_at: Utc::now(),
            executor: Some("Ryze Relayer".into()),
            request: r,
        },
        gross_quote: gross,
    })
}
pub(super) fn empty(r: TradeRequest, id: B256) -> intent::SpotFill {
    intent::SpotFill {
        trade: TradeResult {
            request: r,
            status: TradeStatus::NotFilled,
            filled_qty: Decimal::ZERO,
            avg_price: Decimal::ZERO,
            fees: vec![],
            venue_ref: id.to_string(),
            chain: None,
            finished_at: Utc::now(),
            executor: Some("Ryze Relayer".into()),
        },
        gross_quote: Decimal::ZERO,
    }
}
