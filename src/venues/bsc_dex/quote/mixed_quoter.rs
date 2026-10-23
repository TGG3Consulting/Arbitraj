//! Файл: venues/bsc_dex/quote/mixed_quoter.rs
//! Что делает: сверка умного маршрута перед отправкой — выход каждой доли по MixedQuoter PancakeSwap
//! (quoteMixedExactInput, eth_call на «latest») против локального расчёта доли. Квотер исполняет настоящий обмен
//! (с крючками) и откатывается внутри, поэтому локальный выход сравнивается с учётом влияния крючка.
//! Расхождение больше dex.quoter_max_diff_pct хотя бы у одной доли — сделки нет.
//! Кто вызывает: exec/trade_smart.rs.
//! От чего зависит: node/call.rs, pools/registry.rs, pools/key.rs, quote/quoter.rs (diff_pct).
//! Настройки: smart.mixed_quoter, dex.quoter_max_diff_pct [bsc_dex].
//!
//! Интерфейс и действия: quoteMixedExactInput(address[] paths, bytes actions, bytes[] params, uint256 amountIn)
//! → (amountOut, gasEstimate); V2 = 0x02 (params пустые), V3 = 0x03 (abi.encode(uint24 fee)), INFI_CL = 0x04,
//! INFI_BIN = 0x05 (abi.encode(QuoteMixedInfiExactInputSingleParams{poolKey, hookData})) —
//! https://github.com/pancakeswap/infinity-periphery/blob/56a7a4f5076d57d5ed90c38a00421ecbac0d649c/src/interfaces/IMixedQuoter.sol#L19-L63 ,
//! src/libraries/MixedQuoterActions.sol#L9-L16, src/MixedQuoter.sol#L210-L300; адрес в BSC —
//! research/flow/bsc_smart_router.md, раздел 3.3.

use alloy::sol_types::SolValue;
use alloy_primitives::aliases::U24;
use alloy_primitives::{Address, Bytes, U256};
use rust_decimal::Decimal;

use crate::core::types::PoolKind;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::call;
use super::super::pools::key::PoolKey;
use super::super::pools::registry::Entry;
use super::quoter::diff_pct;

const V2_EXACT_INPUT_SINGLE: u8 = 0x02;
const V3_EXACT_INPUT_SINGLE: u8 = 0x03;
const INFI_CL_EXACT_INPUT_SINGLE: u8 = 0x04;
const INFI_BIN_EXACT_INPUT_SINGLE: u8 = 0x05;

alloy::sol! {
    struct MqKey { address currency0; address currency1; address hooks; address poolManager; uint24 fee; bytes32 parameters; }
    struct QuoteMixedInfiExactInputSingleParams { MqKey poolKey; bytes hookData; }
    interface IMixedQuoter {
        function quoteMixedExactInput(address[] paths, bytes actions, bytes[] params, uint256 amountIn)
            external returns (uint256 amountOut, uint256 gasEstimate);
    }
}

/// Доля для сверки.
pub struct Leg<'a> {
    pub entry: &'a Entry,
    pub token_in: Address,
    pub token_out: Address,
    /// Сколько доходит до пула.
    pub amount_in: U256,
    /// Локальный выход пула (минимальные единицы).
    pub local_out: U256,
    /// Влияние крючка, % (квотер его включает, локальная математика — нет).
    pub hook_pct: Decimal,
}

/// Сверка всех долей. Возвращает: Ok(()) — все в допуске. Ошибки: NotConfigured (адрес, допуск),
/// узел (откат квотера — его текст), QuoteMismatch (расхождение — с цифрами), Internal (у пула V4 нет ключа).
pub async fn check(ctx: &Ctx, legs: &[Leg<'_>]) -> GatewayResult<()> {
    let to = ctx.addr("smart.mixed_quoter")?;
    let limit = ctx.dec("dex.quoter_max_diff_pct")?;
    let calls = legs
        .iter()
        .map(|l| request(ctx, l))
        .collect::<GatewayResult<Vec<_>>>()?;
    let outs = call::view_many(ctx, to, &calls, "latest").await?;
    for (l, out) in legs.iter().zip(outs) {
        let local = less_hook(l.local_out, l.hook_pct);
        match diff_pct(local, out.amountOut) {
            Some(d) if d <= limit => {}
            d => {
                return Err(ctx.err(
                    ErrorCode::QuoteMismatch,
                    format!(
                        "{} {}: расчёт {local} против MixedQuoter {} — расхождение {} > {limit}% (dex.quoter_max_diff_pct)",
                        l.entry.family.title,
                        l.entry.ticker,
                        out.amountOut,
                        d.map_or("не считается".into(), |d| format!("{d}%"))
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// Выход за вычетом влияния крючка: out · (1e6 − pct·1e4) / 1e6 (миллионные доли, как пипсы комиссий). Ошибок нет.
fn less_hook(out: U256, pct: Decimal) -> U256 {
    let million = U256::from(1_000_000u32);
    let ppm = (pct * Decimal::from(10_000u32)).round();
    let ppm = U256::from(u64::try_from(ppm).unwrap_or(0)).min(million);
    out.saturating_mul(million - ppm) / million
}

/// Вызов quoteMixedExactInput для одной доли (один пул). Ошибки: Internal (нет fee у V3 или ключа у V4).
fn request(ctx: &Ctx, l: &Leg<'_>) -> GatewayResult<IMixedQuoter::quoteMixedExactInputCall> {
    let e = l.entry;
    let (action, param) = match e.pool.kind {
        PoolKind::V2 => (V2_EXACT_INPUT_SINGLE, Bytes::new()),
        PoolKind::V3 => {
            let fee = e
                .pool
                .fee_pips
                .ok_or_else(|| ctx.err(ErrorCode::Internal, "у пула V3 нет fee"))?;
            (
                V3_EXACT_INPUT_SINGLE,
                Bytes::from(U24::from(fee).abi_encode()),
            )
        }
        PoolKind::Cl | PoolKind::Bin => {
            let Some(PoolKey::Infinity(k)) = e.key.as_ref() else {
                return Err(ctx.err(ErrorCode::Internal, "у пула Infinity нет ключа"));
            };
            let p = QuoteMixedInfiExactInputSingleParams {
                poolKey: MqKey {
                    currency0: k.currency0,
                    currency1: k.currency1,
                    hooks: k.hooks,
                    poolManager: k.manager,
                    fee: U24::from(k.fee),
                    parameters: k.parameters,
                },
                hookData: Bytes::new(),
            };
            let action = if e.pool.kind == PoolKind::Bin {
                INFI_BIN_EXACT_INPUT_SINGLE
            } else {
                INFI_CL_EXACT_INPUT_SINGLE
            };
            (action, Bytes::from(p.abi_encode()))
        }
    };
    Ok(IMixedQuoter::quoteMixedExactInputCall {
        paths: vec![l.token_in, l.token_out],
        actions: Bytes::from(vec![action]),
        params: vec![param],
        amountIn: l.amount_in,
    })
}
