//! Файл: venues/bsc_dex/quote/quoter.rs
//! Что делает: выход обмена по квотеру своей DEX через eth_call (точный вход) — для сверки локальной математики
//! и для измерения влияния крючка: V2 — Router.getAmountsOut; V3 — QuoterV2; Infinity CL/Bin — CLQuoter/BinQuoter;
//! Uniswap V4 — V4Quoter. Квотер исполняет настоящий обмен (с крючками) и откатывается — research/flow/bsc_v4.md, раздел 5.
//! Сверка: |локально − квотер| / квотер · 100 > dex.quoter_max_diff_pct → пул исключается до следующей сверки.
//! Кто вызывает: check/cycle.rs (сверка по циклу dex.quoter_check_s), check/hooks_probe.rs.
//! От чего зависит: node/call.rs, node/abi_dex.rs, pools/registry.rs, pools/key.rs, quote/pool_quote.rs.
//! Настройки: dex.router (V2), dex.quoter (V3, V4) [вид пула], dex.quoter_max_diff_pct [bsc_dex].
//!
//! Источники: шапка node/abi_dex.rs; «When to use quoter: сверка локального расчёта, финальная проверка» —
//! research/flow/bsc_dex.md, раздел 3, bsc_v4.md, раздел 5.

use alloy_primitives::aliases::{I24, U24, U160};
use alloy_primitives::{Address, Bytes, U256};
use rust_decimal::Decimal;

use crate::core::types::PoolKind;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::abi_dex::{IInfinityQuoter, IQuoterV2, IV2Router, IV4Quoter};
use super::super::node::call;
use super::super::pools::key::PoolKey;
use super::super::pools::registry::Entry;
use super::pool_quote;

/// Выход по квотеру на точный вход. Ошибки: NotConfigured (адрес), узел (откат квотера — его текст), BadResponse.
pub async fn out(ctx: &Ctx, e: &Entry, token_in: Address, amount: U256) -> GatewayResult<U256> {
    let token_out = if token_in == e.pool.token0 {
        e.pool.token1
    } else {
        e.pool.token0
    };
    let zero_for_one = token_in == e.pool.token0;
    match (e.pool.kind, &e.key) {
        (PoolKind::V2, _) => {
            let router = ctx.dex_addr("dex.router", e.family.id)?;
            let r = call::view(
                ctx,
                router,
                &IV2Router::getAmountsOutCall {
                    amountIn: amount,
                    path: vec![token_in, token_out],
                },
                "latest",
            )
            .await?;
            r.last()
                .copied()
                .ok_or_else(|| ctx.err(ErrorCode::BadResponse, "getAmountsOut: пустой ответ"))
        }
        (PoolKind::V3, _) => {
            let q = ctx.dex_addr("dex.quoter", e.family.id)?;
            let fee = U24::from(e.pool.fee_pips.unwrap_or_default());
            let p = IQuoterV2::QuoteExactInputSingleParams {
                tokenIn: token_in,
                tokenOut: token_out,
                amountIn: amount,
                fee,
                sqrtPriceLimitX96: U160::ZERO,
            };
            Ok(call::view(
                ctx,
                q,
                &IQuoterV2::quoteExactInputSingleCall { params: p },
                "latest",
            )
            .await?
            .amountOut)
        }
        (_, Some(PoolKey::Infinity(k))) => {
            let q = ctx.dex_addr("dex.quoter", e.family.id)?;
            let key = IInfinityQuoter::PoolKey {
                currency0: k.currency0,
                currency1: k.currency1,
                hooks: k.hooks,
                poolManager: k.manager,
                fee: U24::from(k.fee),
                parameters: k.parameters,
            };
            let amt: u128 = amount
                .try_into()
                .map_err(|_| ctx.err(ErrorCode::NoRoute, "сумма больше uint128"))?;
            let p = IInfinityQuoter::QuoteExactSingleParams {
                poolKey: key,
                zeroForOne: zero_for_one,
                exactAmount: amt,
                hookData: Bytes::new(),
            };
            Ok(call::view(
                ctx,
                q,
                &IInfinityQuoter::quoteExactInputSingleCall { params: p },
                "latest",
            )
            .await?
            .amountOut)
        }
        (_, Some(PoolKey::Uniswap(k))) => {
            let q = ctx.dex_addr("dex.quoter", e.family.id)?;
            let spacing = I24::try_from(k.tick_spacing)
                .map_err(|_| ctx.err(ErrorCode::BadResponse, "tickSpacing вне int24"))?;
            let key = IV4Quoter::PoolKey {
                currency0: k.currency0,
                currency1: k.currency1,
                fee: U24::from(k.fee),
                tickSpacing: spacing,
                hooks: k.hooks,
            };
            let amt: u128 = amount
                .try_into()
                .map_err(|_| ctx.err(ErrorCode::NoRoute, "сумма больше uint128"))?;
            let p = IV4Quoter::QuoteExactSingleParams {
                poolKey: key,
                zeroForOne: zero_for_one,
                exactAmount: amt,
                hookData: Bytes::new(),
            };
            Ok(call::view(
                ctx,
                q,
                &IV4Quoter::quoteExactInputSingleCall { params: p },
                "latest",
            )
            .await?
            .amountOut)
        }
        _ => Err(ctx.err(
            ErrorCode::Internal,
            format!("{}: у пула V4 нет ключа", e.family.title),
        )),
    }
}

/// Сверка пула: локальная математика против квотера на одной сумме.
/// Принимает: запись, токен входа, сумму. Возвращает: Ok(()) — в пределах допуска; Err(причина) — нет.
/// Ошибки: настройки, узел.
pub async fn compare(
    ctx: &Ctx,
    e: &Entry,
    token_in: Address,
    amount: U256,
) -> GatewayResult<Result<(), String>> {
    let limit = ctx.dec("dex.quoter_max_diff_pct")?;
    let local = pool_quote::swap(ctx, e, token_in, true, amount)?.amount_out;
    let remote = out(ctx, e, token_in, amount).await?;
    let diff = diff_pct(local, remote);
    Ok(match diff {
        Some(d) if d <= limit => Ok(()),
        Some(d) => Err(format!(
            "расчёт {local} против квотера {remote}: расхождение {d}% > {limit}% (dex.quoter_max_diff_pct)"
        )),
        None => Err(format!(
            "расчёт {local} против квотера {remote}: сравнить нельзя"
        )),
    })
}

/// |a − b| / b · 100. None — b = 0 или числа не помещаются в Decimal.
pub fn diff_pct(a: U256, b: U256) -> Option<Decimal> {
    let (a, b) = (to_dec(a)?, to_dec(b)?);
    if b.is_zero() {
        return None;
    }
    Some((a - b).abs() / b * Decimal::ONE_HUNDRED)
}

/// U256 → Decimal без масштаба. None — больше 28 знаков.
fn to_dec(v: U256) -> Option<Decimal> {
    super::super::num::to_decimal(v, 0)
}
