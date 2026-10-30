//! Файл: venues/bsc_dex/quote/mixed_quoter.rs
//! Что делает: сверка умного маршрута перед отправкой — выход каждой доли (один пул или путь через промежуточную
//! монету: paths = токены по порядку, действие и параметры на каждый пул) по MixedQuoter PancakeSwap
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
use alloy_primitives::Bytes;
use alloy_primitives::aliases::U24;

use crate::core::types::PoolKind;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::call;
use super::super::pools::key::PoolKey;
use super::super::pools::registry::Entry;
use super::path::Path;
use super::quoter::{diff_pct, less_hook};

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

/// Доля для сверки: путь (один пул или два через промежуточную монету).
pub struct Leg<'a> {
    pub path: &'a Path,
}

/// Сверка всех долей. Квотер исполняет весь путь доли (с крючками каждого пула); локальный выход последнего пула
/// берётся за вычетом влияния его крючка (влияние крючка первого пула уже учтено во входе второго — quote/via.rs).
/// Возвращает: Ok(()) — все в допуске. Ошибки: NotConfigured (адрес, допуск), узел (откат квотера — его текст),
/// QuoteMismatch (расхождение — с цифрами), Internal (у пула V4 нет ключа, пустой путь).
pub async fn check(ctx: &Ctx, legs: &[Leg<'_>]) -> GatewayResult<()> {
    let to = ctx.addr("smart.mixed_quoter")?;
    let limit = ctx.dec("dex.quoter_max_diff_pct")?;
    let calls = legs
        .iter()
        .map(|l| request(ctx, l.path))
        .collect::<GatewayResult<Vec<_>>>()?;
    let outs = call::view_many(ctx, to, &calls, "latest").await?;
    for (l, out) in legs.iter().zip(outs) {
        let last = l
            .path
            .hops
            .last()
            .ok_or_else(|| ctx.err(ErrorCode::Internal, "доля без пулов"))?;
        let local = less_hook(last.swapped.amount_out, last.usable.hook_pct).ok_or_else(|| {
            ctx.err(
                ErrorCode::QuoteMismatch,
                format!("влияние крючка {}% не считается", last.usable.hook_pct),
            )
        })?;
        match diff_pct(local, out.amountOut) {
            Some(d) if d <= limit => {}
            d => {
                return Err(ctx.err(
                    ErrorCode::QuoteMismatch,
                    format!(
                        "{} {}: расчёт {local} против MixedQuoter {} — расхождение {} > {limit}% (dex.quoter_max_diff_pct)",
                        last.entry.family.title,
                        last.entry.ticker,
                        out.amountOut,
                        d.map_or("не считается".into(), |d| format!("{d}%"))
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// Вызов quoteMixedExactInput для доли: путь токенов, действие и параметры на каждый пул, вход первого пула.
/// Ошибки: Internal (пустой путь, нет fee у V3 или ключа у V4).
fn request(ctx: &Ctx, p: &Path) -> GatewayResult<IMixedQuoter::quoteMixedExactInputCall> {
    let first = p
        .hops
        .first()
        .ok_or_else(|| ctx.err(ErrorCode::Internal, "доля без пулов"))?;
    let mut paths = vec![first.token_in];
    let (mut actions, mut params) = (Vec::new(), Vec::new());
    for h in &p.hops {
        let (a, b) = hop(ctx, &h.entry)?;
        paths.push(h.token_out);
        actions.push(a);
        params.push(b);
    }
    Ok(IMixedQuoter::quoteMixedExactInputCall {
        paths,
        actions: Bytes::from(actions),
        params,
        amountIn: first.swapped.amount_in,
    })
}

/// Действие и параметры одного пула. Ошибки: Internal (нет fee у V3 или ключа у V4).
fn hop(ctx: &Ctx, e: &Entry) -> GatewayResult<(u8, Bytes)> {
    Ok(match e.pool.kind {
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
        // DODO PMM (решение 86b): не входит в умный маршрут Universal Router (не tradable, исполнение — 86c).
        PoolKind::Pmm => {
            return Err(ctx.err(
                ErrorCode::Internal,
                "DODO PMM не исполняется умным маршрутом (фаза 86c)",
            ));
        }
    })
}
