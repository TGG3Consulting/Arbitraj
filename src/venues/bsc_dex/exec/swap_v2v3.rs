//! Файл: venues/bsc_dex/exec/swap_v2v3.rs
//! Что делает: входы команд Universal Router V2_SWAP_EXACT_IN и V3_SWAP_EXACT_IN для обмена в одном пуле:
//! abi.encode(recipient, amountIn, amountOutMin, path, payerIsUser[, minHopPriceX36]). Путь V2 — address[];
//! путь V3 — упакованные байты tokenIn(20) | fee(3) | tokenOut(20). Платит пользователь (payerIsUser = true, Permit2)
//! или роутер — второй шаг пути через промежуточную монету (payerIsUser = false, amountIn = CONTRACT_BALANCE:
//! весь баланс роутера, research/flow/bsc_smart_router.md, разделы 2.5 и 3.1).
//! Кто вызывает: exec/plan.rs, exec/universal.rs.
//! От чего зависит: exec/ur.rs (Format), pools/registry.rs.
//! Настройки: нет.
//!
//! PancakeSwap: Dispatcher.sol#L61-L76 (V3), #L167-L182 (V2) — pancake-v4-universal-router 1b6d479e;
//! Uniswap: contracts/base/Dispatcher.sol#L68-L84, #L187-L200 (universal-router edf19bfd) и SDK routerCommands.ts:
//! в 2.0 входы без minHopPriceX36, с 2.1.1 — с пустым массивом по умолчанию (uniswap.ts#L613, #L661).
//! V2-обмен UR считает вход пары как balanceOf(pair) − reserve и проверяет выход по балансу получателя —
//! работает и с монетами с налогом на перевод (V2SwapRouter.sol, оба UR).

use alloy::sol_types::SolValue;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::PoolKind;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::pools::registry::Entry;
use super::plan::Order;
use super::ur::{Format, V2_SWAP_EXACT_IN, V3_SWAP_EXACT_IN};

/// Команда и её вход. Принимает: пул (V2/V3), формат, обмен (токены, вход, минимум выхода), получателя (кошелёк или
/// ADDRESS_THIS — роутер), платит ли кошелёк (false — роутер из своего баланса).
/// Ошибки: Internal — пул не V2/V3 или у V3 нет fee.
pub fn command(
    ctx: &Ctx,
    e: &Entry,
    f: Format,
    o: &Order,
    to: Address,
    payer_user: bool,
) -> GatewayResult<(u8, Bytes)> {
    let (tin, tout, amount_in, min_out) = (o.token_in, o.token_out, o.amount_in, o.min_out);
    let min_hop = f == Format::Uniswap211;
    let empty: Vec<U256> = Vec::new();
    match e.pool.kind {
        PoolKind::V2 => {
            let path = vec![tin, tout];
            let input = if min_hop {
                (to, amount_in, min_out, path, payer_user, empty).abi_encode_params()
            } else {
                (to, amount_in, min_out, path, payer_user).abi_encode_params()
            };
            Ok((V2_SWAP_EXACT_IN, Bytes::from(input)))
        }
        PoolKind::V3 => {
            let fee = e
                .pool
                .fee_pips
                .ok_or_else(|| ctx.err(ErrorCode::Internal, "у пула V3 нет fee"))?;
            let mut path = Vec::with_capacity(43);
            path.extend_from_slice(tin.as_slice());
            path.extend_from_slice(&fee.to_be_bytes()[1..]);
            path.extend_from_slice(tout.as_slice());
            let path = Bytes::from(path);
            let input = if min_hop {
                (to, amount_in, min_out, path, payer_user, empty).abi_encode_params()
            } else {
                (to, amount_in, min_out, path, payer_user).abi_encode_params()
            };
            Ok((V3_SWAP_EXACT_IN, Bytes::from(input)))
        }
        _ => Err(ctx.err(ErrorCode::Internal, format!("{}: не V2/V3", e.family.title))),
    }
}
