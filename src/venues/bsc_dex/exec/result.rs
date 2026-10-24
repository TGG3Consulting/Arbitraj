//! Файл: venues/bsc_dex/exec/result.rs
//! Что делает: итог сделки DEX по квитанции — статус транзакции (в блоке / откат / не попала / вытеснена) и факт
//! обмена по событиям Transfer: сколько монет и USDT реально ушло и пришло, средняя цена, газ.
//! Кто вызывает: exec/trade.rs.
//! От чего зависит: exec/fill.rs, exec/txs.rs (Done), num.rs, core/types.
//! Настройки: нет.
//!
//! «Факт: квитанция → реальные суммы из логов Transfer/Swap → сверка с планом» — research/flow/bsc_dex.md, схема п. 8.

use alloy_primitives::Address;
use chrono::Utc;
use rust_decimal::Decimal;

use crate::core::types::{Side, TradeRequest, TradeResult, TradeStatus, TxOutcome};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::num;
use super::super::wallet::balances::BNB;
use super::fill;
use super::txs;

/// Итог сделки по квитанции. Ошибки: SwapReverted / TxNotIncluded с фактом транзакции; OutcomeUnknown (ядро не
/// повторяет, площадка на паузу) — «вытеснена» или обмен в блоке, а суммы не считаются (сырые числа и факт «в блоке»).
/// Принимает: запрос, итог транзакции, кошелёк, (токен входа, токен выхода), (decimals монеты, decimals USDT).
pub fn result(
    ctx: &Ctx,
    req: TradeRequest,
    d: txs::Done,
    wallet: Address,
    (tin, tout): (Address, Address),
    (coin_dec, usdt_dec): (u8, u8),
) -> GatewayResult<TradeResult> {
    let r = match (d.chain.outcome, &d.receipt) {
        (TxOutcome::Included, Some(r)) => r.clone(),
        (TxOutcome::Reverted, _) => {
            return Err(ctx
                .err(
                    ErrorCode::SwapReverted,
                    format!("Обмен откатился в блоке (газ сгорел), tx {}", d.chain.hash),
                )
                .with_tx(d.chain));
        }
        (TxOutcome::Replaced, _) => {
            return Err(ctx
                .err(
                    ErrorCode::OutcomeUnknown,
                    format!("Nonce занят, а своей квитанции нет за send.replaced_confirm_blocks — исход обмена tx {} неизвестен, нужна ручная проверка", d.chain.hash),
                )
                .with_tx(d.chain));
        }
        _ => {
            return Err(ctx
                .err(
                    ErrorCode::TxNotIncluded,
                    format!(
                        "Транзакция не попала в блок за send.max_wait_blocks, tx {}{}",
                        d.chain.hash,
                        d.cancel.as_ref().map_or(String::new(), |(h, _)| format!(
                            " — nonce занят отменой {h}"
                        ))
                    ),
                )
                .with_tx(d.chain));
        }
    };
    let spent = fill::moved(&r, tin, Some(wallet), None);
    let got = fill::moved(&r, tout, None, Some(wallet));
    let (coins_u, usdt_u) = if req.side == Side::Buy {
        (got, spent)
    } else {
        (spent, got)
    };
    // Обмен уже в блоке: факт не теряется — если сумма не помещается в Decimal, ошибка с сырыми числами и фактом
    // транзакции (статус «в блоке» — по квитанции), а не тихий 0 («не исполнена» → риск повтора).
    let (Some(coins), Some(usdt), Some(fee)) = (
        num::to_decimal(coins_u, coin_dec),
        num::to_decimal(usdt_u, usdt_dec),
        d.chain.gas_bnb,
    ) else {
        return Err(ctx
            .err(
                ErrorCode::OutcomeUnknown,
                format!(
                    "Обмен в блоке, tx {}, но суммы не переводятся в числа: монет {coins_u} (decimals {coin_dec}), USDT {usdt_u} (decimals {usdt_dec}), газ {:?}",
                    d.chain.hash, d.chain.gas_bnb
                ),
            )
            .with_tx(d.chain));
    };
    let status = if coins.is_zero() {
        TradeStatus::NotFilled
    } else if coins < req.qty {
        TradeStatus::PartiallyFilled
    } else {
        TradeStatus::Filled
    };
    Ok(TradeResult {
        status,
        filled_qty: coins,
        avg_price: if coins.is_zero() {
            Decimal::ZERO
        } else {
            usdt / coins
        },
        fee,
        fee_asset: BNB.to_string(),
        venue_ref: d.chain.hash.to_string(),
        chain: Some(d.chain),
        finished_at: Utc::now(),
        request: req,
    })
}
