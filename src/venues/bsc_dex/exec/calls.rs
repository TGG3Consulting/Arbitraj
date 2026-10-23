//! Файл: venues/bsc_dex/exec/calls.rs
//! Что делает: данные вызова прямого обмена в пуле V2 или V3 через роутер своей DEX — для пробного обмена
//! (check/probe.rs) и для настоящей сделки (exec/*, шаг 1d).
//! V2: swapExactTokensForTokens или …SupportingFeeOnTransferTokens (монеты с налогом); PancakeSwap V3:
//! SwapRouter.exactInputSingle (deadline в структуре); Uniswap V3: SwapRouter02.multicall(deadline, [exactInputSingle]).
//! Кто вызывает: check/probe.rs, позже exec/v2.rs, exec/v3.rs.
//! От чего зависит: node/abi_dex.rs, pools/registry.rs, pools/families.rs.
//! Настройки: нет (роутер и deadline передаёт вызывающий: dex.router, dex.deadline_s).
//!
//! Источники — шапка node/abi_dex.rs; FOT-функция V2 — research/flow/bsc_dex.md, раздел 5 и 6;
//! deadline SwapRouter02 — MulticallExtended.sol#L14-L18 (swap-router-contracts 70bc2e40).

use alloy::sol_types::SolCall;
use alloy_primitives::aliases::{U24, U160};
use alloy_primitives::{Address, Bytes, U256};

use crate::gateway::errors::{ErrorCode, GatewayError};

use super::super::node::abi_dex::{IPancakeSwapRouterV3, IUniswapSwapRouter02, IV2Router};
use super::super::pools::families::Proto;
use super::super::pools::registry::Entry;

/// Параметры обмена одним пулом.
pub struct Swap {
    pub token_in: Address,
    pub token_out: Address,
    pub amount_in: U256,
    pub min_out: U256,
    pub recipient: Address,
    /// Срок (секунды Unix).
    pub deadline: U256,
    /// V2: функция для монет с налогом на перевод.
    pub fee_on_transfer: bool,
}

/// Данные вызова роутера. Принимает: запись пула, параметры. Возвращает: calldata.
/// Ошибки: Internal — у пула не V2/V3 (V4 — через Universal Router, exec/v4.rs) или нет fee у V3.
pub fn direct(e: &Entry, s: &Swap) -> Result<Bytes, GatewayError> {
    let bad = |m: &str| {
        GatewayError::new(
            ErrorCode::Internal,
            None,
            format!("{}: {m}", e.family.title),
        )
    };
    let data = match e.family.proto {
        Proto::V2 => {
            let path = vec![s.token_in, s.token_out];
            if s.fee_on_transfer {
                IV2Router::swapExactTokensForTokensSupportingFeeOnTransferTokensCall {
                    amountIn: s.amount_in,
                    amountOutMin: s.min_out,
                    path,
                    to: s.recipient,
                    deadline: s.deadline,
                }
                .abi_encode()
            } else {
                IV2Router::swapExactTokensForTokensCall {
                    amountIn: s.amount_in,
                    amountOutMin: s.min_out,
                    path,
                    to: s.recipient,
                    deadline: s.deadline,
                }
                .abi_encode()
            }
        }
        Proto::PancakeV3 => {
            let fee = U24::from(e.pool.fee_pips.ok_or_else(|| bad("нет fee"))?);
            IPancakeSwapRouterV3::exactInputSingleCall {
                params: IPancakeSwapRouterV3::ExactInputSingleParams {
                    tokenIn: s.token_in,
                    tokenOut: s.token_out,
                    fee,
                    recipient: s.recipient,
                    deadline: s.deadline,
                    amountIn: s.amount_in,
                    amountOutMinimum: s.min_out,
                    sqrtPriceLimitX96: U160::ZERO,
                },
            }
            .abi_encode()
        }
        Proto::UniswapV3 => {
            let fee = U24::from(e.pool.fee_pips.ok_or_else(|| bad("нет fee"))?);
            let inner = IUniswapSwapRouter02::exactInputSingleCall {
                params: IUniswapSwapRouter02::ExactInputSingleParams {
                    tokenIn: s.token_in,
                    tokenOut: s.token_out,
                    fee,
                    recipient: s.recipient,
                    amountIn: s.amount_in,
                    amountOutMinimum: s.min_out,
                    sqrtPriceLimitX96: U160::ZERO,
                },
            }
            .abi_encode();
            IUniswapSwapRouter02::multicallCall {
                deadline: s.deadline,
                data: vec![Bytes::from(inner)],
            }
            .abi_encode()
        }
        _ => return Err(bad("прямой обмен V4 — через Universal Router (шаг 1d)")),
    };
    Ok(Bytes::from(data))
}
