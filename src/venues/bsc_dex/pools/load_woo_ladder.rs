//! Файл: venues/bsc_dex/pools/load_woo_ladder.rs
//! Что делает: «лесенка цен пула» WooFi (решение 122 F, 3; выбор Тиграна) на блоке — ответы квотера самого контракта
//! `query(from, to, amount)` (IWooPPV2.sol L91) на лесенке объёмов dex.pmm_ladder_usd в обе стороны: проход 1 —
//! котируемая (USDT) → базовая на суммах лесенки; проход 2 — базовая → котируемая на полученных в проходе 1 количествах
//! базовой (тот же объём в долларах). Оба прохода — Multicall3 на одном блоке. Откат квотера в точке (пределы пула) —
//! край лесенки: точек после него нет. Своих формул пула нет — только ответы квотера.
//! Кто вызывает: pools/load_woo.rs.
//! От чего зависит: ctx.rs, node/call.rs, tokens.rs, abi_woo.rs, core/types (LadderPoint).
//! Настройки: dex.pmm_ladder_usd [DEX] (не задана — лесенки нет, пара остаётся сигналом).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use crate::core::types::LadderPoint;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::call;
use super::super::tokens::Tokens;
use super::abi_woo::IWooPP;

/// Лесенка в долларах из dex.pmm_ladder_usd: суммы > 0 по возрастанию. Принимает: окружение, id DEX.
/// Возвращает: список; пусто — настройка не задана. Ошибки: NotConfigured — значение не число > 0 или не по возрастанию.
pub fn usd(ctx: &Ctx, id: &str) -> GatewayResult<Vec<Decimal>> {
    let list = match ctx.dex_list("dex.pmm_ladder_usd", id) {
        Ok(l) => l,
        Err(e) if e.code == ErrorCode::NotConfigured => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out: Vec<Decimal> = Vec::new();
    for s in &list {
        let v: Decimal = s.trim().parse().map_err(|_| {
            ctx.not_set(
                "dex.pmm_ladder_usd",
                format!("«{s}» — не число (сумма USDT)"),
            )
        })?;
        if v <= Decimal::ZERO || out.last().is_some_and(|l| *l >= v) {
            return Err(ctx.not_set(
                "dex.pmm_ladder_usd",
                "суммы лесенки — больше нуля и по возрастанию",
            ));
        }
        out.push(v);
    }
    Ok(out)
}

/// Обе стороны лесенки на блоке. Принимает: окружение, токены, id DEX, контракт, базовую, котируемую, USDT, тег.
/// Возвращает: (котируемая → базовая, базовая → котируемая); пусто — лесенки нет (не задана; котируемая не USDT).
/// Ошибки: узел, ответ не по ABI, неверная dex.pmm_ladder_usd.
#[allow(clippy::too_many_arguments)]
pub async fn read(
    ctx: &Ctx,
    tokens: &Tokens,
    id: &str,
    pool: Address,
    coin: Address,
    quote: Address,
    usdt: Address,
    tag: &str,
) -> GatewayResult<(Vec<LadderPoint>, Vec<LadderPoint>)> {
    let ladder = usd(ctx, id)?;
    if ladder.is_empty() || quote != usdt {
        return Ok((Vec::new(), Vec::new()));
    }
    let dec = tokens.decimals(ctx, quote).await?;
    let unit = Decimal::from(10u64.pow(u32::from(dec)));
    let amounts: Vec<U256> = ladder
        .iter()
        .filter_map(|v| (*v * unit).trunc().to_u128().map(U256::from))
        .collect();
    let quote_in = pass(ctx, pool, quote, coin, &amounts, tag).await?;
    let base_amounts: Vec<U256> = quote_in.iter().map(|p| p.amount_out).collect();
    let base_in = pass(ctx, pool, coin, quote, &base_amounts, tag).await?;
    Ok((quote_in, base_in))
}

/// Один проход: query(from, to, amount) по суммам; точки — до первого отката или нулевого выхода. Ошибки: узел.
async fn pass(
    ctx: &Ctx,
    pool: Address,
    from: Address,
    to: Address,
    amounts: &[U256],
    tag: &str,
) -> GatewayResult<Vec<LadderPoint>> {
    let calls: Vec<(Address, Bytes)> = amounts
        .iter()
        .map(|a| {
            let q = IWooPP::queryCall {
                fromToken: from,
                toToken: to,
                fromAmount: *a,
            };
            (pool, q.abi_encode().into())
        })
        .collect();
    let got = call::try_many(ctx, &calls, tag).await?;
    let mut out = Vec::new();
    for (a, r) in amounts.iter().zip(got) {
        let Ok(b) = r else { break };
        let o = call::decode::<IWooPP::queryCall>(&b)?;
        if o.is_zero() || a.is_zero() || out.last().is_some_and(|p: &LadderPoint| p.amount_in >= *a)
        {
            break;
        }
        out.push(LadderPoint {
            amount_in: *a,
            amount_out: o,
        });
    }
    Ok(out)
}
