//! Файл: venues/bsc_dex/exec/executor/sim.rs
//! Что делает: проба вызова исполнителя прямо перед отправкой (решение 122, часть X; как exec/precheck_sim.rs у
//! Universal Router): execute симулируется eth_simulateV1 от кошелька поверх блока сборки; откат — сделки нет (текст
//! узла; «deadline» — StaleData с подсказкой про dex.deadline_s); получено (возврат execute = прирост выхода у
//! кошелька, меряет сам контракт) сверяется с req.worst_price (exec/order.rs::check_price). Проверка «кто меняет»
//! (решение 59) здесь не нужна: исполнитель вызывает пул ровно по адресу шага, а не ищет его по фабрике.
//! Кто вызывает: exec/executor/trade.rs.
//! От чего зависит: check/simulate.rs, exec/executor/{abi, call}.rs, exec/order.rs, num.rs.
//! Настройки: нет (узел — bsc.node_http_url).
//!
//! eth_simulateV1 — execution-apis 16caee27 src/eth/execute.yaml#L137-L260 (CallResult.returnData).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, U256};

use crate::core::types::{Side, TradeRequest};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::super::check::simulate::{self, Call, CallResult};
use super::super::super::ctx::Ctx;
use super::super::super::num;
use super::super::order::check_price;
use super::abi::ArbExecutor;
use super::call::ExecCall;

/// Проба execute. Принимает: запрос, вызов, кошелёк, номер блока сборки, (decimals монеты, USDT).
/// Возвращает: сколько получит кошелёк (единицы токена выхода) и газ вызова.
/// Ошибки: NoRoute (откат), StaleData (срок вышел), QuoteMismatch (цена хуже req.worst_price), BadResponse, узел.
pub async fn check(
    ctx: &Ctx,
    req: &TradeRequest,
    c: &ExecCall,
    wallet: Address,
    at: u64,
    decs: (u8, u8),
) -> GatewayResult<(U256, u64)> {
    let call = Call {
        from: wallet,
        to: c.executor,
        input: c.data(),
    };
    let res = simulate::run(ctx, &[call], at).await?;
    let r = res
        .first()
        .ok_or_else(|| ctx.err(ErrorCode::BadResponse, "eth_simulateV1: нет итога вызова"))?;
    let got = received(ctx, r)?;
    price(ctx, req, c, got, decs)?;
    Ok((got, r.gas_used.to::<u64>()))
}

/// Возврат execute из итога симуляции. Ошибки: NoRoute (откат), StaleData (срок), BadResponse (не разбирается).
pub fn received(ctx: &Ctx, r: &CallResult) -> GatewayResult<U256> {
    if !r.ok() {
        let text = r
            .error
            .as_ref()
            .map(|e| e.message.as_str())
            .unwrap_or_default();
        // require(block.timestamp <= deadline, "deadline") — время симулированного блока у BSC +12 с (check/simulate.rs).
        if text.contains("deadline") {
            let why = format!(
                "проба исполнителя: срок транзакции вышел — увеличьте dex.deadline_s: {}",
                r.why()
            );
            return Err(ctx.err(ErrorCode::StaleData, why));
        }
        return Err(ctx.err(
            ErrorCode::NoRoute,
            format!("проба исполнителя откатилась — {}", r.why()),
        ));
    }
    ArbExecutor::executeCall::abi_decode_returns(&r.return_data).map_err(|e| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("возврат execute не разбирается: {e}"),
        )
    })
}

/// Сверка с худшей ценой: покупка — USDT входа / монет получено, продажа — USDT получено / монет входа.
/// Ошибки: QuoteMismatch (хуже req.worst_price), NoRoute (монет 0), BadResponse (числа).
fn price(
    ctx: &Ctx,
    req: &TradeRequest,
    c: &ExecCall,
    got: U256,
    (coin_dec, usdt_dec): (u8, u8),
) -> GatewayResult<()> {
    let bad = || ctx.err(ErrorCode::BadResponse, "сумма пробы не переводится в число");
    let (coins, usdt) = match req.side {
        Side::Buy => (
            num::to_decimal(got, coin_dec),
            num::to_decimal(c.amount_in, usdt_dec),
        ),
        Side::Sell => (
            num::to_decimal(c.amount_in, coin_dec),
            num::to_decimal(got, usdt_dec),
        ),
    };
    check_price(ctx, req, coins.ok_or_else(bad)?, usdt.ok_or_else(bad)?)
}
