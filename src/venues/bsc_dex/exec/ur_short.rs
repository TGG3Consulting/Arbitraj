//! Файл: venues/bsc_dex/exec/ur_short.rs
//! Что делает: решение 248 (кейс SMARS 10.11: «проба сделки перед отправкой: шаг 0 откатился — код 3: execution
//! reverted: 0x849eaf98») — распознаёт откат Universal Router «мало на выходе» (минимум выхода обмена не набран) по
//! селектору custom error в данных отката или в тексте узла. Селекторы — keccak256 сигнатуры ошибки из официального
//! исходника (считает alloy sol!, проверка — tests/exit_ur_248.rs):
//! - V2TooLittleReceived() = 0x849eaf98 — PancakeSwap pancake-v4-universal-router 1b6d479e
//!   src/modules/pancakeswap/v2/V2SwapRouter.sol#L14 (объявление), #L75 (`if (amountOut < amountOutMinimum) revert
//!   V2TooLittleReceived();`); Uniswap universal-router edf19bfd contracts/modules/uniswap/v2/V2SwapRouter.sol#L13, #L94;
//! - V3TooLittleReceived() = 0x39d35496 — PancakeSwap src/modules/pancakeswap/v3/V3SwapRouter.sol#L26, #L115; Uniswap
//!   contracts/modules/uniswap/v3/V3SwapRouter.sol#L25, #L159;
//! - StableTooLittleReceived() = 0xcd7d97e4 — PancakeSwap src/modules/pancakeswap/StableSwapRouter.sol#L21, #L108;
//! - TooLittleReceived(uint256,uint256) = 0x4e86d23a — PancakeSwap infinity-periphery 56a7a4f5
//!   src/interfaces/IInfinityRouter.sol#L14 (откат — src/pool-cl/CLRouterBase.sol#L37, #L64, src/pool-bin/
//!   BinRouterBase.sol#L37, #L63, src/InfinityRouter.sol#L88);
//! - V4TooLittleReceived(uint256,uint256) = 0x8b063d73 — Uniswap v4-periphery 0c4e5bd3 src/interfaces/IV4Router.sol#L13
//!   (откат — src/V4Router.sol#L64, #L93, #L132);
//! - ExecutionFailed(uint256,bytes) = 0x2c4029e9 с сообщением BalanceTooLow() = 0xa3281672 — проверка баланса
//!   BALANCE_CHECK_ERC20 не набрала минимум: Dispatcher ставит success = false и BalanceTooLow (pancake-v4-universal-
//!   router src/base/Dispatcher.sol#L29, #L247-L259), execute откатывает ExecutionFailed (src/UniversalRouter.sol#L62-L63,
//!   src/interfaces/IUniversalRouter.sol#L6); Uniswap — contracts/base/Dispatcher.sol#L37, #L263-L274,
//!   contracts/UniversalRouter.sol#L84-L85.
//!
//! Свопы V2/V3 вызываются из dispatch напрямую (pancake-v4-universal-router src/base/Dispatcher.sol#L182, #L76) — их откат приходит как есть, без
//! обёртки ExecutionFailed (так и в журнале SMARS: «execution reverted: 0x849eaf98»).
//! Где селектор: CallResult.returnData (execution-apis 16caee27 src/schemas/execute.yaml CallResultFailure) или текст
//! узла «execution reverted: 0x…» (факт журнала бота; у eth_simulateV1 узла BSC returnData отката в журнале не записан).
//! Кто вызывает: exec/ur_remeasure.rs.
//! От чего зависит: alloy (sol!, SolError).
//! Настройки: нет.

use alloy::sol_types::SolError;
use alloy_primitives::hex;

alloy::sol! {
    /// Universal Router V2 (PancakeSwap и Uniswap): выход ниже amountOutMinimum.
    error V2TooLittleReceived();
    /// Universal Router V3: выход ниже amountOutMinimum.
    error V3TooLittleReceived();
    /// Universal Router PancakeSwap StableSwap: выход ниже amountOutMinimum.
    error StableTooLittleReceived();
    /// PancakeSwap Infinity (IInfinityRouter): выход ниже amountOutMinimum.
    error TooLittleReceived(uint256 minAmountOutReceived, uint256 amountReceived);
    /// Uniswap v4 (IV4Router): выход ниже amountOutMinimum.
    error V4TooLittleReceived(uint256 minAmountOutReceived, uint256 amountReceived);
    /// BALANCE_CHECK_ERC20: баланс получателя ниже минимума (сообщение ExecutionFailed).
    error BalanceTooLow();
    /// execute: команда без флага «можно откатиться» не прошла (IUniversalRouter).
    error ExecutionFailed(uint256 commandIndex, bytes message);
}

/// Ошибки «мало на выходе», которые приходят как есть: (селектор, имя для журнала).
pub const DIRECT: [([u8; 4], &str); 5] = [
    (V2TooLittleReceived::SELECTOR, "V2TooLittleReceived"),
    (V3TooLittleReceived::SELECTOR, "V3TooLittleReceived"),
    (StableTooLittleReceived::SELECTOR, "StableTooLittleReceived"),
    (TooLittleReceived::SELECTOR, "TooLittleReceived"),
    (V4TooLittleReceived::SELECTOR, "V4TooLittleReceived"),
];

/// Откат «мало на выходе» Universal Router: имя ошибки, если данные отката начинаются с её селектора или текст узла
/// содержит его hex; ExecutionFailed — только с сообщением BalanceTooLow (проверка баланса). Иные откаты (срок,
/// налог-контракт монеты, нет ликвидности) — None. Принимает: данные отката, текст узла. Ошибок нет.
pub fn too_little(data: &[u8], message: &str) -> Option<&'static str> {
    let text = message.to_ascii_lowercase();
    let has = |sel: &[u8; 4]| data.starts_with(sel) || text.contains(&hex::encode(sel));
    if let Some((_, name)) = DIRECT.iter().find(|(sel, _)| has(sel)) {
        return Some(name);
    }
    let low = BalanceTooLow::SELECTOR;
    let wrapped = ExecutionFailed::abi_decode(data).is_ok_and(|e| e.message.starts_with(&low));
    let in_text =
        text.contains(&hex::encode(ExecutionFailed::SELECTOR)) && text.contains(&hex::encode(low));
    (wrapped || in_text).then_some("ExecutionFailed(BalanceTooLow)")
}
