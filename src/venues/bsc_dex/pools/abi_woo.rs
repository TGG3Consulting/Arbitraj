//! Файл: venues/bsc_dex/pools/abi_woo.rs
//! Что делает: ABI WooFi WooPPV2 и его оракула по публичным официальным интерфейсам (решение 122 F, 3): котируемая
//! монета, оракул пула, пауза, квотер `query` / `tryQuery`, событие обмена `WooSwap`; оракул — `state(base)`; прокси —
//! событие `Upgraded` и слот реализации EIP-1967. Только эти вызовы: раскладка `tokenInfos` у развёрнутой реализации
//! другая, чем в публичном коде (поле capBal) — её не читаем, пока нет проверенного исходника.
//! Кто вызывает: pools/load_woo.rs, pools/verify_woo.rs, pools/events_woo.rs, pools/factory_ask.rs, quote/quoter_calls.rs.
//! От чего зависит: alloy::sol!, alloy-primitives.
//! Настройки: нет (адреса — dex.pmm_pool, dex.oracle, dex.pool_implementation).
//!
//! Источники (официальные, решение 23): woonetwork/WooPoolV2 f73a0f64 — contracts/interfaces/IWooPPV2.sol: WooSwap
//! L51-L61, quoteToken L67, tryQuery L79, query L91, swap; contracts/WooPPV2.sol: `wooracle` L88 (public), Pausable
//! (OpenZeppelin `paused()`); contracts/interfaces/IWooracleV2_2.sol: struct State {uint128 price; uint64 spread;
//! uint64 coeff; bool woFeasible}, `state(address)`. Прокси: «WooPPV2.2» на BSC — TransparentUpgradeableProxy
//! (woonetwork/docs 68dbdad references/readme/bnb-smart-chain.md); слот реализации и событие Upgraded — EIP-1967
//! (eips.ethereum.org/EIPS/eip-1967: bytes32(uint256(keccak256('eip1967.proxy.implementation')) − 1),
//! `event Upgraded(address indexed implementation)`).

// Код вызовов и событий генерирует alloy::sol! — число полей задаёт ABI контракта, а не мы.
#![allow(clippy::too_many_arguments)]

use alloy_primitives::{B256, U256, keccak256};

alloy::sol! {
    /// Общий контракт WooPPV2 (публичный интерфейс).
    interface IWooPP {
        event WooSwap(address indexed fromToken, address indexed toToken, uint256 fromAmount, uint256 toAmount, address from, address indexed to, address rebateTo, uint256 swapVol, uint256 swapFee);
        event Upgraded(address indexed implementation);
        event Paused(address account);
        event Unpaused(address account);
        event WooracleUpdated(address indexed newWooracle);

        function quoteToken() external view returns (address);
        function wooracle() external view returns (address);
        function paused() external view returns (bool);
        function query(address fromToken, address toToken, uint256 fromAmount) external view returns (uint256 toAmount);
        function tryQuery(address fromToken, address toToken, uint256 fromAmount) external view returns (uint256 toAmount);
    }

    /// Оракул WooFi (IWooracleV2_2.State).
    interface IWooracle {
        struct State { uint128 price; uint64 spread; uint64 coeff; bool woFeasible; }
        function state(address base) external view returns (State memory);
    }
}

/// Слот реализации прокси EIP-1967: keccak256("eip1967.proxy.implementation") − 1. Ошибок нет.
pub fn implementation_slot() -> B256 {
    let h = U256::from_be_bytes(keccak256(b"eip1967.proxy.implementation").0);
    B256::from((h - U256::from(1u8)).to_be_bytes::<32>())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::sol_types::{SolCall, SolEvent};

    /// Селекторы, topic0 и слот совпадают с официальными сигнатурами и EIP-1967.
    #[test]
    fn signatures_match_official() {
        let sel = |sig: &str| keccak256(sig.as_bytes())[..4].to_vec();
        let topic = |sig: &str| B256::from(keccak256(sig.as_bytes()));
        assert_eq!(
            IWooPP::queryCall::SELECTOR.to_vec(),
            sel("query(address,address,uint256)")
        );
        assert_eq!(
            IWooracle::stateCall::SELECTOR.to_vec(),
            sel("state(address)")
        );
        assert_eq!(
            IWooPP::WooSwap::SIGNATURE_HASH,
            topic(
                "WooSwap(address,address,uint256,uint256,address,address,address,uint256,uint256)"
            )
        );
        assert_eq!(IWooPP::Upgraded::SIGNATURE_HASH, topic("Upgraded(address)"));
        assert_eq!(
            format!("{:x}", implementation_slot()),
            "360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc"
        );
    }
}
