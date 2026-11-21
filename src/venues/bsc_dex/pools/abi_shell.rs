//! Файл: venues/bsc_dex/pools/abi_shell.rs
//! Что делает: ABI Shell Protocol (решение 486, вид shell_proteus): пул LiquidityPoolProxy — ocean / xToken / yToken /
//! implementation / poolFrozen и события пула; реализация Proteus — getSlopes / getAs / getBs / getKs / BASE_FEE /
//! swapGivenInputAmount (сверка выхода); учётный контракт Ocean — balanceOfBatch, unwrapFeeDivisor; токен —
//! decimals. id токена в учёте: keccak256(abi.encodePacked(адрес, uint256 0)) (у ERC-20 id = 0), нативная монета —
//! адрес 0x4574686572 («Ether» в ASCII) — константа протокола.
//! Кто вызывает: pools/{load_shell, verify_shell, events_shell, shell_list}.rs, quote/quoter_shell.rs.
//! От чего зависит: alloy::sol!, alloy-primitives.
//! Настройки: нет.
//!
//! Источник (решение 23): Sourcify 42161 (exact_match) — LiquidityPoolProxy 0xC32A9fC5… (v3) L14-L37, LiquidityPool
//! L29-L36, L67-L70 (события Swap / Deposit / Withdraw / InitialLiquidity), Proteus / Slices getSlopes / getAs / getBs /
//! getKs, BASE_FEE; LiquidityPoolProxy v2 — Shell-Protocol/Shell-Protocol@bfbba01 src/proteus/LiquidityPoolProxy.sol
//! L10-L21 (те же подписи); Ocean — aaa0ccf src/ocean/Ocean.sol L96 (unwrapFeeDivisor), L197 (WRAPPED_ETHER_ID),
//! OceanERC1155.sol L307-L313 (_calculateOceanId), Interactions.sol (Interaction, InteractionType); v3 0x96B4f4E4…
//! Ocean.sol L110, L139, OceanERC1155.sol L242-L243 — те же.

// Код функций и событий генерирует alloy::sol! — число аргументов задаёт ABI контракта, а не мы.
#![allow(clippy::too_many_arguments)]

use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, B256, U256, address, keccak256};

/// Адрес-метка нативной монеты учёта: hexadecimal(ascii("Ether")) (Ocean.sol aaa0ccf L197) — константа протокола.
pub const ETHER_MARK: Address = address!("0x0000000000000000000000000000004574686572");

alloy::sol! {
    interface IShellPool {
        function ocean() external view returns (address);
        function xToken() external view returns (uint256);
        function yToken() external view returns (uint256);
        function implementation() external view returns (address);
        function poolFrozen() external view returns (bool);
        event Swap(uint256 inputToken, uint256 inputAmount, uint256 outputAmount, bytes32 slippageProtection, address user, bool computeOutput);
        event Deposit(uint256 inputToken, uint256 inputAmount, uint256 outputAmount, bytes32 slippageProtection, address user, bool computeOutput);
        event Withdraw(uint256 outputToken, uint256 inputAmount, uint256 outputAmount, bytes32 slippageProtection, address user, bool computeOutput);
        event InitialLiquidity(uint256 inputToken, uint256 inputAmount, uint256 outputAmount, address user, bool active);
        event ImplementationChanged(address operator, address oldImplementation, address newImplementation);
        event PoolFrozen(address operator);
    }

    interface IProteus {
        function getSlopes() external view returns (int128[] memory);
        function getAs() external view returns (int128[] memory);
        function getBs() external view returns (int128[] memory);
        function getKs() external view returns (int128[] memory);
        function BASE_FEE() external view returns (uint256);
        function swapGivenInputAmount(uint256 xBalance, uint256 yBalance, uint256 inputAmount, uint8 inputToken) external view returns (uint256);
    }

    interface IShellOcean {
        function balanceOfBatch(address[] accounts, uint256[] ids) external view returns (uint256[] memory);
        function unwrapFeeDivisor() external view returns (uint256);
    }

    interface IShellToken {
        function decimals() external view returns (uint8);
    }
}

/// id токена в учёте: keccak256(abi.encodePacked(token, uint256(0))) (_calculateOceanId). Ошибок нет.
pub fn ocean_id(token: Address) -> U256 {
    let mut b = [0u8; 52];
    b[..20].copy_from_slice(token.as_slice());
    U256::from_be_bytes(keccak256(b).0)
}

/// id нативной монеты учёта (WRAPPED_ETHER_ID). Ошибок нет.
pub fn ether_id() -> U256 {
    ocean_id(ETHER_MARK)
}

/// topic0 всех событий пула: любое меняет балансы, реализацию или заморозку — перечитать пул. Ошибок нет.
pub fn topics() -> Vec<B256> {
    vec![
        IShellPool::Swap::SIGNATURE_HASH,
        IShellPool::Deposit::SIGNATURE_HASH,
        IShellPool::Withdraw::SIGNATURE_HASH,
        IShellPool::InitialLiquidity::SIGNATURE_HASH,
        IShellPool::ImplementationChanged::SIGNATURE_HASH,
        IShellPool::PoolFrozen::SIGNATURE_HASH,
    ]
}

/// topic0 событий изменения ликвидности (внесение, снятие, первое внесение). Ошибок нет.
pub fn liquidity_topics() -> Vec<B256> {
    vec![
        IShellPool::Deposit::SIGNATURE_HASH,
        IShellPool::Withdraw::SIGNATURE_HASH,
        IShellPool::InitialLiquidity::SIGNATURE_HASH,
    ]
}
