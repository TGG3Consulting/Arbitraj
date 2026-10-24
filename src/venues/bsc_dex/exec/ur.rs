//! Файл: venues/bsc_dex/exec/ur.rs
//! Что делает: кодирование вызова Universal Router своей DEX — команды и их входы для прямого обмена в одном пуле:
//! V2_SWAP_EXACT_IN (0x08), V3_SWAP_EXACT_IN (0x00), V4_SWAP / INFI_SWAP (0x10) и проверка реально полученного
//! баланса BALANCE_CHECK_ERC20 (0x0e) в той же транзакции (решение 40: меньше минимума — откат всей транзакции);
//! execute(commands, inputs, deadline). Формат входов зависит от DEX и версии Universal Router Uniswap.
//! Нативная монета (решение 62): WRAP_ETH (0x0b) и UNWRAP_WETH (0x0c) — вход abi.encode(recipient, amount) у обоих UR:
//! PancakeSwap — Commands.sol#L28-L29, Dispatcher.sol#L218-L238, Payments.sol#L76-L104 (wrapETH: CONTRACT_BALANCE или
//! точная сумма ≤ баланса BNB роутера; unwrapWETH9: весь WBNB роутера, меньше amountMinimum — откат InsufficientETH);
//! Uniswap — contracts/libraries/Commands.sol#L28-L29, contracts/base/Dispatcher.sol#L237-L256,
//! contracts/modules/Payments.sol#L91-L119 (то же). WETH9 роутера в BSC = WBNB 0xbb4CdB9C… — параметры развёртывания:
//! PancakeSwap script/deployParameters/mainnet/DeployBsc.s.sol#L24, Uniswap script/deployParameters/DeployBSC.s.sol#L11.
//! Кто вызывает: exec/build.rs, check/probe.rs.
//! От чего зависит: exec/swap_v2v3.rs, exec/swap_v4.rs, pools/registry.rs, ctx.rs.
//! Настройки: dex.universal_router_version [DEX]: «pancakeswap_infinity», «2.0», «2.1.1», «2.1.2».
//!
//! Источники: PancakeSwap — https://github.com/pancakeswap/pancake-v4-universal-router/blob/1b6d479e1f76361f5950d1168d9392361e73a1e8/src/libraries/Commands.sol#L15-L35 ,
//! src/base/Dispatcher.sol#L61-L76, #L167-L182, #L247-L259; execute — src/UniversalRouter.sol#L33.
//! Uniswap — https://github.com/Uniswap/universal-router/blob/edf19bfde87e6cf5b29046f6ca2f483a4ffd6ae6/contracts/libraries/Commands.sol#L15-L35 ;
//! входы по версиям (2.0 — без minHopPriceX36, 2.1.1+ — с ним) — официальный SDK:
//! https://github.com/Uniswap/sdks/blob/ecffab39cb8eba0ceb1a8c0052fd287f7765fcc8/sdks/universal-router-sdk/src/utils/routerCommands.ts#L135-L297 .

use alloy::sol_types::{SolCall, SolValue};
use alloy_primitives::{Address, Bytes, U256};

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::pools::registry::Entry;

/// Команды Universal Router (Commands.sol, одинаковые номера у PancakeSwap и Uniswap).
pub const V3_SWAP_EXACT_IN: u8 = 0x00;
pub const V2_SWAP_EXACT_IN: u8 = 0x08;
pub const WRAP_ETH: u8 = 0x0b;
pub const UNWRAP_WETH: u8 = 0x0c;
pub const BALANCE_CHECK_ERC20: u8 = 0x0e;
pub const V4_SWAP: u8 = 0x10;

/// «Весь баланс роутера» как сумма входа — ActionConstants.CONTRACT_BALANCE = 1 << 255: infinity-periphery 56a7a4f5
/// src/libraries/ActionConstants.sol#L14, v4-periphery 0c4e5bd3 src/libraries/ActionConstants.sol#L13 (их используют
/// оба Universal Router: Payments.sol, V3SwapRouter.sol).
pub const CONTRACT_BALANCE: U256 = U256::from_limbs([0, 0, 0, 0x8000_0000_0000_0000]);
/// Получатель «сам роутер» — ActionConstants.ADDRESS_THIS = address(2): там же, #L20 / #L19; map() в Dispatcher.sol.
pub const ADDRESS_THIS: Address = Address::with_last_byte(2);

alloy::sol! {
    interface IUniversalRouter {
        function execute(bytes calldata commands, bytes[] calldata inputs, uint256 deadline) external payable;
    }
}

/// Формат входов команд.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// PancakeSwap Infinity Universal Router.
    Pancake,
    /// Uniswap Universal Router 2.0 (без minHopPriceX36).
    Uniswap20,
    /// Uniswap Universal Router 2.1.1 и 2.1.2 (с minHopPriceX36).
    Uniswap211,
}

/// Формат по настройке dex.universal_router_version DEX пула. Ошибки: NotConfigured (версия не задана или неизвестна).
pub fn format(ctx: &Ctx, e: &Entry) -> GatewayResult<Format> {
    match ctx.dex_text("dex.universal_router_version", e.family.id)?.trim() {
        "pancakeswap_infinity" => Ok(Format::Pancake),
        "2.0" => Ok(Format::Uniswap20),
        "2.1.1" | "2.1.2" => Ok(Format::Uniswap211),
        v => Err(ctx.err(
            ErrorCode::NotConfigured,
            format!("dex.universal_router_version [{}] = «{v}»: известны pancakeswap_infinity, 2.0, 2.1.1, 2.1.2", e.family.id),
        )),
    }
}

/// Вход BALANCE_CHECK_ERC20: abi.encode(owner, token, minBalance). Ошибок нет.
pub fn balance_check(owner: Address, token: Address, min_balance: U256) -> Bytes {
    Bytes::from((owner, token, min_balance).abi_encode_params())
}

/// Вход WRAP_ETH / UNWRAP_WETH: abi.encode(recipient, amount) (Dispatcher.sol обоих UR). Ошибок нет.
pub fn native(recipient: Address, amount: U256) -> Bytes {
    Bytes::from((recipient, amount).abi_encode_params())
}

/// Данные execute(commands, inputs, deadline). Принимает: команды с входами, срок. Ошибок нет.
pub fn execute(steps: Vec<(u8, Bytes)>, deadline: U256) -> Bytes {
    let (commands, inputs): (Vec<u8>, Vec<Bytes>) = steps.into_iter().unzip();
    let call = IUniversalRouter::executeCall {
        commands: Bytes::from(commands),
        inputs,
        deadline,
    };
    Bytes::from(call.abi_encode())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Селектор execute(bytes,bytes[],uint256) = 0x3593564c (research/flow/bsc_v4.md, раздел 7).
    #[test]
    fn execute_selector() {
        assert_eq!(
            alloy_primitives::hex::encode(IUniversalRouter::executeCall::SELECTOR),
            "3593564c"
        );
    }
}
