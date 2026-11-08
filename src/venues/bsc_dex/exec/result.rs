//! Файл: venues/bsc_dex/exec/result.rs
//! Что делает: итог сделки DEX по квитанции — статус транзакции (в блоке / откат / не попала / вытеснена) и факт
//! обмена по событиям Transfer: сколько монет и USDT реально ушло и пришло, средняя цена, газ.
//! Кто вызывает: exec/trade_direct.rs, exec/trade_smart.rs, exec/bundle_trade.rs, exec/executor/fact.rs; подпись исполнителя (by, решение
//! 121 B7) — exec/trade.rs.
//! От чего зависит: exec/fill.rs, exec/txs.rs (Done), num.rs, core/types.
//! Настройки: нет.
//! Нативная монета BNB (решение 62): монеты — по событиям WBNB Deposit / Withdrawal роутера (роутеров бандла — решение
//! 122 R3; exec/native.rs, exec/bundle_native.rs), USDT — по Transfer.
//!
//! «Факт: квитанция → реальные суммы из логов Transfer/Swap → сверка с планом» — research/flow/bsc_dex.md, схема п. 8.

use alloy_primitives::Address;
use chrono::Utc;
use rust_decimal::Decimal;

use crate::core::types::{Fee, Side, TradeRequest, TradeResult, TradeStatus, TxOutcome};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::num;
use super::super::wallet::balances::BNB;
use super::txs;
use super::{bundle_native, fill};

/// Кто исполнил своп (решение 121, часть B7) — подпись в TradeResult.executor для журнала и хронологии сделки.
pub const BY_DIRECT: &str = "свой сборщик: прямой";
/// Умный путь: все доли одним execute Universal Router PancakeSwap (exec/trade_smart.rs).
pub const BY_SMART: &str = "свой сборщик: умный";
/// Мульти-роутерный бандл свопов (exec/bundle_trade.rs, решение 84).
pub const BY_BUNDLE: &str = "свой сборщик: бандл";
/// Одна транзакция через свой контракт-исполнитель ArbExecutor (exec/executor/trade.rs, решение 122, часть X).
pub const BY_EXECUTOR: &str = "свой сборщик: исполнитель";

/// Итог сделки с подписью исполнителя (решение 121 B7): успех — поле executor; ошибка (откат, не попала в блок, отказ
/// до отправки) — подпись в начале текста «[свой сборщик: …] …», код и факт транзакции не меняются. Принимает: итог,
/// подпись. Решение 177: пометка исполнения, уже стоящая в executor («… — продажа по квотеру», exec/exit_quote.rs),
/// остаётся после подписи через «; ». Возвращает: тот же итог с подписью. Ошибок своих нет.
pub fn by(r: GatewayResult<TradeResult>, who: &str) -> GatewayResult<TradeResult> {
    match r {
        Ok(mut t) => {
            t.executor = Some(match t.executor.take() {
                Some(note) => format!("{who}; {note}"),
                None => who.to_string(),
            });
            Ok(t)
        }
        Err(mut e) => {
            e.message = format!("[{who}] {}", e.message);
            Err(e)
        }
    }
}

/// Итог сделки по квитанции. Ошибки: SwapReverted / TxNotIncluded с фактом транзакции; OutcomeUnknown (ядро не
/// повторяет, площадка на паузу) — «вытеснена» или обмен в блоке, а суммы не считаются (сырые числа и факт «в блоке»).
/// Принимает: запрос, итог транзакции, кошелёк, (токен входа, токен выхода), (decimals монеты, decimals USDT),
/// роутеры — у нативной монеты (монеты считаются по обёртке/развороту WBNB этими роутерами; у бандла — все его
/// Universal Router, решение 122 R3), иначе пусто.
pub fn result(
    ctx: &Ctx,
    req: TradeRequest,
    d: txs::Done,
    wallet: Address,
    (tin, tout): (Address, Address),
    (coin_dec, usdt_dec): (u8, u8),
    native_routers: &[Address],
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
            // «Чем найти» с хешами отмен (позднее выяснение — resolve_single.rs: чья квитанция заняла nonce).
            let seed = super::swap_ctx::with_cancels(
                super::swap_ctx::seed(&d.chain, d.tip_bnb),
                &d.cancels,
            );
            return Err(ctx
                .err(
                    ErrorCode::OutcomeUnknown,
                    format!("Nonce занят, а своей квитанции нет за send.replaced_confirm_blocks — исход обмена tx {} неизвестен, нужна ручная проверка", d.chain.hash),
                )
                .with_tx(d.chain)
                .pending(seed));
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
    // Нативная BNB: на кошелёк приходит (или уходит с value) нативный BNB — событий Transfer кошелька нет.
    let coins_u = if native_routers.is_empty() {
        coins_u
    } else {
        let wbnb = if req.side == Side::Buy { tout } else { tin };
        bundle_native::moved_all(&r, wbnb, native_routers, req.side)
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
        fees: Fee::single(fee, BNB),
        venue_ref: d.chain.hash.to_string(),
        chain: Some(d.chain),
        finished_at: Utc::now(),
        executor: None,
        request: req,
    })
}
