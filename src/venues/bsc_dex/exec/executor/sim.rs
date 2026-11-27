//! Файл: venues/bsc_dex/exec/executor/sim.rs
//! Что делает: проба вызова исполнителя прямо перед отправкой (решение 122, часть X; как exec/precheck_sim.rs у
//! Universal Router): execute симулируется eth_simulateV1 от кошелька поверх блока сборки; откат — сделки нет (текст
//! узла; «deadline» — StaleData с подсказкой про dex.deadline_s); получено (возврат execute = прирост выхода у
//! кошелька, меряет сам контракт) сверяется с req.worst_price (exec/order.rs::check_price). Проверка «кто меняет»
//! (решение 59) здесь не нужна: исполнитель вызывает пул ровно по адресу шага, а не ищет его по фабрике.
//! Решение 401 (t186): откат пробы (не срок, не minOut) помечает пулы шагов менеджера вызова «исполнитель отвергнут» до
//! следующей перепроверки цикла крючков (exec/executor/pool_pass.rs on_sim) — бот не натыкается на них снова.
//! Решение 234 (п. 4; живой случай ASS 08.11 — откат «minOut» без чисел): к ошибке пробы (откат, срок, цена) —
//! minOut и ожидаемый выход каждой доли, модельный налог монеты на её шаге (legs_text, explain); текст ошибки продажи
//! уходит в хронологию сделки (core/sell.rs → manual_outcome / алерт).
//! Кто вызывает: exec/executor/trade.rs.
//! От чего зависит: check/simulate.rs, exec/executor/{abi, call, pool_pass}.rs, exec/order.rs, num.rs.
//! Настройки: нет (узел — bsc.node_http_url).
//!
//! eth_simulateV1 — execution-apis 16caee27 src/eth/execute.yaml#L137-L260 (CallResult.returnData).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::{Side, TradeRequest};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::super::super::check::simulate::{self, Call, CallResult};
use super::super::super::ctx::Ctx;
use super::super::super::num;
use super::super::chain::Step;
use super::super::order::check_price;
use super::abi::ArbExecutor;
use super::call::ExecCall;

/// Проба execute. Принимает: запрос, вызов, кошелёк, (номер, время) блока сборки — симулированный блок на 1 с позже
/// (check/simulate.rs Base::timed), (decimals монеты, USDT).
/// Возвращает: сколько получит кошелёк (единицы токена выхода) и газ вызова.
/// Ошибки: NoRoute (откат), StaleData (срок вышел), QuoteMismatch (цена хуже req.worst_price), BadResponse, узел.
pub async fn check(
    ctx: &Ctx,
    req: &TradeRequest,
    c: &ExecCall,
    wallet: Address,
    (at, now): (u64, u64),
    decs: (u8, u8),
) -> GatewayResult<(U256, u64)> {
    let r = run(ctx, c, wallet, (at, now)).await?;
    judge(ctx, req, c, &r, wallet, decs)
}

/// Одна симуляция execute от кошелька поверх блока сборки (симулированный блок на 1 с позже). Решение 247: итог как
/// есть — откат по minOut распознаёт exec/executor/remeasure.rs. Ошибки: узел, BadResponse (нет итога).
pub async fn run(
    ctx: &Ctx,
    c: &ExecCall,
    wallet: Address,
    (at, now): (u64, u64),
) -> GatewayResult<CallResult> {
    let call = Call {
        from: wallet,
        to: c.executor,
        input: c.data(),
    };
    // Решение 272: продажа нативного BNB (executeNative, wrapIn) — с value вызова; иначе value нет, как раньше.
    let value = c.value();
    let values: &[(usize, U256)] = if value.is_zero() { &[] } else { &[(0, value)] };
    let res = simulate::run_valued(ctx, &[call], values, simulate::Base::for_chain(ctx.chain_id(), at, now)).await?;
    let r = res
        .into_iter()
        .next()
        .ok_or_else(|| ctx.err(ErrorCode::BadResponse, "eth_simulateV1: нет итога вызова"))?;
    // Решение 401: откат (не срок, не minOut) — пулы шагов менеджера помечаются «отвергнут» до перепроверки
    // (синхронно, без await на пути сделки).
    super::pool_pass::on_sim(ctx.place.id, c, &r);
    Ok(r)
}

/// Разбор итога пробы: получено (откат — ошибка received) и сверка с req.worst_price. Возвращает: получено и газ.
/// Ошибки: как check.
pub fn judge(
    ctx: &Ctx,
    req: &TradeRequest,
    c: &ExecCall,
    r: &CallResult,
    wallet:Address,
    decs: (u8, u8),
) -> GatewayResult<(U256, u64)> {
    let got = received(ctx, r)?;
    let debit=llamma_debit(c,wallet,&r.logs).unwrap_or(c.amount_in);
    price(ctx, req, debit, got, decs)?;
    Ok((got, r.gas_used.to::<u64>()))
}

/// Возврат execute из итога симуляции (у executeNative — тот же тип возврата uint256, решение 272: разбор общий). Ошибки: NoRoute (откат), StaleData (срок), BadResponse (не разбирается).
pub fn received(ctx: &Ctx, r: &CallResult) -> GatewayResult<U256> {
    if !r.ok() {
        let text = r
            .error
            .as_ref()
            .map(|e| e.message.as_str())
            .unwrap_or_default();
        // require(block.timestamp <= deadline, "deadline") — симулированный блок: время головы + 1 с (check/simulate.rs).
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
/// Возврат входной монеты учитывается только для первой ноги LLAMMA, подтверждённой реестром.
fn llamma_debit(c:&ExecCall,wallet:Address,logs:&[super::super::super::check::simulate::SimLog])->Option<U256>{
    use alloy::sol_types::{SolEvent,SolValue};
    use crate::venues::bsc_dex::{exec::{fill,signed_call},pools::llamma::ILlama};
    if !c.first_step_llamma { return None; }
    let first=c.steps.first()?;
    if c.native.is_some()||first.kind!=9||first.tokenIn!=c.token_in||first.shareBps!=10_000||
        c.steps.iter().skip(1).any(|s|s.tokenIn==c.token_in||s.tokenOut==c.token_in){return None}
    let (i,j)=<(U256,U256)>::abi_decode(&first.data).ok()?;
    if !((i==U256::ZERO&&j==U256::ONE)||(j==U256::ZERO&&i==U256::ONE)){return None}
    let logs=signed_call::logs(logs)?;
    let mut hits=logs.iter().filter(|l|l.address()==first.pool&&l.topic0()==Some(&ILlama::TokenExchange::SIGNATURE_HASH));
    let event=ILlama::TokenExchange::decode_log_data(&hits.next()?.inner.data).ok()?;
    if hits.next().is_some()||event.sold_id!=i||event.bought_id!=j||event.recipient!=c.executor||event.tokens_bought.is_zero(){return None}
    let flow=|src,to|fill::moved_logs(&logs,c.token_in,Some(src),Some(to));
    let paid=flow(wallet,c.executor);let refunded=flow(c.executor,wallet);let charged=paid.checked_sub(refunded)?;
    (paid==c.amount_in&&!refunded.is_zero()&&!charged.is_zero()&&flow(c.executor,first.pool)==charged&&event.tokens_sold==charged).then_some(charged)
}

fn price(
    ctx: &Ctx,
    req: &TradeRequest,
    amount_in:U256,
    got: U256,
    (coin_dec, usdt_dec): (u8, u8),
) -> GatewayResult<()> {
    let bad = || ctx.err(ErrorCode::BadResponse, "сумма пробы не переводится в число");
    let (coins, usdt) = match req.side {
        Side::Buy => (
            num::to_decimal(got, coin_dec),
            num::to_decimal(amount_in, usdt_dec),
        ),
        Side::Sell => (
            num::to_decimal(amount_in, coin_dec),
            num::to_decimal(got, usdt_dec),
        ),
    };
    check_price(ctx, req, coins.ok_or_else(bad)?, usdt.ok_or_else(bad)?)
}

/// Что делает: текст «доля k: minOut M, ожидалось E, модельный налог T%» по долям — для отката пробы (решение 234, п. 4).
/// Принимает: шаги долей (минимум — у последнего шага, exec/leg_min.rs), по долям (ожидаемый выход, налог %), запрос
/// (сторона: продажа — USDT, покупка — монеты), (decimals монеты, USDT). Ошибок нет (число не переводится — «?»).
pub fn legs_text(
    legs: &[Vec<Step>],
    exp: &[(Decimal, Decimal)],
    req: &TradeRequest,
    (coin_dec, usdt_dec): (u8, u8),
) -> String {
    let (dec, unit) = match req.side {
        Side::Sell => (usdt_dec, "USDT"),
        Side::Buy => (coin_dec, req.asset.ticker.as_str()),
    };
    let parts: Vec<String> = legs
        .iter()
        .zip(exp)
        .enumerate()
        .map(|(k, (leg, (e, tax)))| {
            let min = leg
                .last()
                .and_then(|s| num::to_decimal(s.order.min_out, dec))
                .map_or("?".to_string(), |m| m.normalize().to_string());
            format!(
                "доля {}: minOut {min} {unit}, ожидалось {} {unit}, модельный налог монеты {}%",
                k + 1,
                e.round_dp(6).normalize(),
                tax.round_dp(4).normalize()
            )
        })
        .collect();
    parts.join("; ")
}

/// Что делает: дописывает к ошибке пробы текст по долям (legs_text). Принимает: ошибку, текст. Возвращает: ошибку с
/// дополненным текстом (код и факт транзакции те же). Ошибок нет.
pub fn explain(mut e: GatewayError, legs: &str) -> GatewayError {
    if !legs.is_empty() {
        e.message = format!("{}; {legs}", e.message);
    }
    e
}

#[cfg(test)]
#[path = "llamma_debit_tests.rs"]
mod llamma_debit_tests;
