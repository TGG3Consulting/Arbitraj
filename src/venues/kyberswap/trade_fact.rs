//! Файл: venues/kyberswap/trade_fact.rs
//! Что делает: факт свопа KyberSwap по квитанции — суммы из логов Transfer кошелька (EIP-20 событие Transfer), средняя
//! цена, газ, статус исполнения; подпись исполнителя «KyberSwap» (решение 121 B7). Вынесено из trade.rs (решение 123:
//! trade.rs разделён на «получить маршрут» — trade_prep.rs и «исполнить» — trade.rs).
//! Кто вызывает: trade.rs (execute), transfer.rs (moved).
//! От чего зависит: abi.rs, node.rs (Receipt), tx.rs (Done), num.rs, balances.rs, core/types.
//! Настройки: нет.

use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, U256};
use chrono::Utc;
use rust_decimal::Decimal;

use crate::core::types::{Side, TradeRequest, TradeResult, TradeStatus};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::abi::IERC20;
use super::ctx::Ctx;
use super::node::Receipt;
use super::{num, tx};

/// Подпись исполнителя в TradeResult.executor и в начале текста ошибок свопа (решение 121 B7).
pub const BY_KYBER: &str = "KyberSwap";

/// Факт свопа по квитанции: суммы из логов Transfer кошелька, средняя цена, газ. Принимает: окружение, запрос, итог
/// транзакции, квитанцию, кошелёк, (токен входа, токен выхода), (decimals монеты, decimals USDT).
/// Ошибки: OutcomeUnknown (суммы не переводятся в числа — с фактом транзакции).
pub fn result(
    ctx: &Ctx,
    req: TradeRequest,
    d: tx::Done,
    r: &Receipt,
    wallet: Address,
    (tin, tout): (Address, Address),
    (coin_dec, usdt_dec): (u8, u8),
) -> GatewayResult<TradeResult> {
    let spent = moved(r, tin, Some(wallet), None);
    let got = moved(r, tout, None, Some(wallet));
    let (coins_u, usdt_u) = if req.side == Side::Buy {
        (got, spent)
    } else {
        (spent, got)
    };
    let (Some(coins), Some(usdt), Some(fee)) = (
        num::to_decimal(coins_u, coin_dec),
        num::to_decimal(usdt_u, usdt_dec),
        d.chain.gas_bnb,
    ) else {
        return Err(ctx
            .err(
                ErrorCode::OutcomeUnknown,
                format!(
                    "Своп в блоке, tx {}, но суммы не переводятся в числа",
                    d.chain.hash
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
        fee_asset: super::balances::BNB.to_string(),
        venue_ref: d.chain.hash.to_string(),
        chain: Some(d.chain),
        finished_at: Utc::now(),
        // Решение 121 B7: кто исполнил своп — для журнала и хронологии сделки.
        executor: Some(BY_KYBER.to_string()),
        request: req,
    })
}

/// Сумма переводов токена по квитанции (события Transfer с нужными from/to). Ошибок нет.
pub fn moved(r: &Receipt, token: Address, from: Option<Address>, to: Option<Address>) -> U256 {
    r.logs
        .iter()
        .filter(|l| l.address() == token)
        .filter_map(|l| IERC20::Transfer::decode_log_data(&l.inner.data).ok())
        .filter(|t| from.is_none_or(|f| t.from == f) && to.is_none_or(|x| t.to == x))
        .fold(U256::ZERO, |acc, t| acc.saturating_add(t.value))
}
