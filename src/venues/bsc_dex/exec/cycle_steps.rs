//! Файл: venues/bsc_dex/exec/cycle_steps.rs
//! Что делает: общие части исполнения цикла DEX ↔ DEX (решение 124) для исполнителя, бандла и сухой сделки:
//! - запрос свопа в формате ядра (TradeRequest «базовая → базовая»: сторона Sell, qty = вход, худшая цена = minOut / вход
//!   — граница ядра в единицах «выход за 1 входа»);
//! - шаги исполнителя для цикла одной долей (exec/chain.rs Step): первый шаг берёт весь вход у кошелька, последний несёт
//!   minOut ядра — у ArbExecutor это minOut execute (contracts/ArbExecutor.sol: при tokenIn = tokenOut прямой вход
//!   выключен, вход забирается до замера баланса, received = весь выход цикла);
//! - проверка чистой прибыли ПОСЛЕ симуляции: выход − вход − (газ симуляции × цена газа + чаевые + газ перевода
//!   чаевых — send/tip_gas.rs, Тигран 09.11) в базовой ≥
//!   dex_dex.min_profit_usd, иначе сделки нет (газ съел прибыль); чаевые и их газ — только у формы, которая их шлёт
//!   (исполнитель; у бандла — нет, plan_tip; Тигран 09.11: согласовано, план = факт);
//! - итог сделки цикла (CycleTradeResult) из итога свопа.
//!
//! Кто вызывает: exec/{cycle_trade, cycle_exec, cycle_bundle_trade, cycle_dry}.rs, quote/cycle_cost.rs (plan_tip).
//! От чего зависит: quote/{cycle, cycle_cost, route_calc}.rs, exec/{chain, plan}.rs, send/mod.rs, num.rs, core/types.
//! Настройки: dex_dex.min_profit_usd [bsc_dex] (обязательна для исполнения); через send — channel.* (чаевые).

use alloy_primitives::U256;
use rust_decimal::Decimal;

use crate::core::types::{
    Asset, CycleTradeRequest, CycleTradeResult, Side, TradeRequest, TradeResult,
};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::num;
use super::super::quote::cycle_cost::Rates;
use super::super::quote::route_calc::Leg;
use super::super::quote::{Quoting, cycle};
use super::super::send;
use super::chain::Step;
use super::plan::Order;

/// Своп цикла в формате ядра: «продать базовую за базовую» на входе req.input не хуже req.min_out. Ошибок нет.
pub fn trade_request(req: &CycleTradeRequest) -> TradeRequest {
    let worst = if req.input.is_zero() {
        Decimal::ZERO
    } else {
        req.min_out / req.input
    };
    TradeRequest {
        venue: req.venue.clone(),
        asset: Asset {
            ticker: req.cycle.base_ticker.clone(),
            bsc_contract: Some(req.cycle.base),
        },
        side: Side::Sell,
        qty: req.input,
        worst_price: worst,
        deal_id: req.deal_id.clone(),
        route: None,
        // Решение 177: цикл DEX ↔ DEX — не выход из позиции, сверка с квотером — стоп, как раньше.
        exit_position: false,
    }
}

/// Единицы базовой монеты. Ошибки: BadResponse — не переводится.
pub fn units(ctx: &Ctx, v: Decimal, dec: u8) -> GatewayResult<U256> {
    num::to_units(v, dec)
        .ok_or_else(|| ctx.err(ErrorCode::BadResponse, "сумма цикла не в единицах токена"))
}

/// Шаги цикла одной долей: вход у кошелька, промежуточные — весь баланс исполнителя, последний — кошельку с minOut.
/// Принимает: ноги, вход и minOut (базовая). Возвращает: шаги и ожидаемый выход. Ошибки: NoRoute (шаг не исполним
/// целиком), BadResponse, QuoteMismatch (minOut 0).
pub fn exec_steps(
    ctx: &Ctx,
    legs: &[Leg],
    input: Decimal,
    min_out: Decimal,
) -> GatewayResult<(Vec<Step>, Decimal)> {
    let path = cycle::eval(ctx, legs, input)?;
    let n = path.hops.len();
    let mut steps = Vec::with_capacity(n);
    for (j, h) in path.hops.iter().enumerate() {
        let last = j + 1 == n;
        let amount_in = units(ctx, h.pool_in, h.dec_in)?;
        let min = if last {
            units(ctx, min_out, h.dec_out)?
        } else {
            units(ctx, h.pool_out, h.dec_out)?
        };
        if last && min.is_zero() {
            return Err(ctx.err(ErrorCode::QuoteMismatch, "minOut цикла 0 — сделки нет"));
        }
        steps.push(Step {
            entry: h.entry.clone(),
            order: Order {
                token_in: h.token_in,
                token_out: h.token_out,
                amount_in: if j == 0 {
                    units(ctx, input, h.dec_in)?
                } else {
                    amount_in
                },
                min_out: min,
            },
            from_wallet: j == 0,
            to_wallet: last,
        });
    }
    Ok((steps, path.usdt))
}

/// Цена газа как при отправке: eth_gasPrice с надбавкой, не ниже минимума готовых каналов. Ошибки: узел, каналы.
pub async fn wei(q: &Quoting) -> GatewayResult<U256> {
    Ok(q.gas
        .wei(&q.ctx, &q.heads)
        .await?
        .max(send::planned_floor(&q.ctx)?))
}

/// Чаевые канала и газ их перевода (BNB) в плане цикла. Тигран 09.11 (согласовано, план = факт): только у формы,
/// которая их отправляет, — исполнитель одной транзакцией (exec/cycle_exec.rs → exec/txs.rs send с чаевыми); бандл
/// свопов (exec/cycle_bundle_trade.rs → send::dispatch_slice) их не шлёт — ноль, узел не спрашивается.
/// Принимает: окружение, цену газа, шлёт ли форма чаевые. Возвращает: (чаевые, газ перевода чаевых); чаевых в плане
/// нет (send::planned_tip) — нули. Ошибки: как send/tip_gas.rs (ключ кошелька, узел eth_estimateGas, BadResponse).
pub async fn plan_tip(ctx: &Ctx, wei: U256, with_tip: bool) -> GatewayResult<(Decimal, Decimal)> {
    if !with_tip {
        return Ok((Decimal::ZERO, Decimal::ZERO));
    }
    let tip = send::planned_tip(ctx)
        .and_then(|w| num::to_decimal(w, num::BNB_DECIMALS))
        .unwrap_or(Decimal::ZERO);
    let tip_gas = send::tip_gas::planned_bnb(ctx, wei)
        .await?
        .unwrap_or(Decimal::ZERO);
    Ok((tip, tip_gas))
}

/// Чистая прибыль по симуляции. Принимает: выход и вход (базовая), газ симуляции (единиц), цену газа, курсы, шлёт ли
/// форма исполнения чаевые (исполнитель — да, бандл — нет; plan_tip).
/// Возвращает: чистую прибыль в базовой. Ошибки: NotConfigured (dex_dex.min_profit_usd), QuoteMismatch — ниже порога,
/// BadResponse (числа); газ перевода чаевых — как send/tip_gas.rs (ключ кошелька, узел eth_estimateGas).
/// Тигран 09.11 (согласовано): к чаевым — газ их отдельной транзакции перевода (send/tip_gas.rs, та же цена газа).
pub async fn net_check(
    ctx: &Ctx,
    (output, input): (Decimal, Decimal),
    (gas_units, wei): (u64, U256),
    r: &Rates,
    with_tip: bool,
) -> GatewayResult<Decimal> {
    // Порог — в USDT (решение 124): одна настройка для любой базовой монеты, в базовую — по курсу пулов.
    let min_usd = ctx.dec("dex_dex.min_profit_usd")?;
    let min_profit = min_usd / r.base_usdt;
    let (tip, tip_gas) = plan_tip(ctx, wei, with_tip).await?;
    let gas = num::to_decimal(wei.saturating_mul(U256::from(gas_units)), num::BNB_DECIMALS)
        .ok_or_else(|| ctx.err(ErrorCode::BadResponse, "газ симуляции не в Decimal"))?;
    let net = output - input - (gas + tip + tip_gas) * r.bnb_in_base;
    if net < min_profit {
        let why = format!(
            "по симуляции чистая прибыль цикла {net} (≈ {} USDT) < порога dex_dex.min_profit_usd {min_usd} (выход {output}, \
             вход {input}, газ {gas_units} ед., чаевые {tip} BNB, газ перевода чаевых {tip_gas} BNB) — сделки нет",
            (net * r.base_usdt).round_dp(6)
        );
        return Err(ctx.err(ErrorCode::QuoteMismatch, why));
    }
    Ok(net)
}

/// Итог сделки цикла из итога свопа (сторона Sell: qty = вход, avg_price = выход / вход). Ошибок нет.
pub fn finish(t: TradeResult, r: &Rates) -> CycleTradeResult {
    CycleTradeResult {
        input: t.filled_qty,
        output: t.filled_qty * t.avg_price,
        base_usdt: r.base_usdt,
        bnb_usdt: r.bnb_usdt,
        trade: t,
    }
}
