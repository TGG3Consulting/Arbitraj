//! Файл: venues/bsc_dex/pools/abi_smardex.rs
//! Что делает: ABI пары SmarDex (решение 288, вид smardex): геттеры состояния (getReserves — баланс минус доля протокола,
//! getFictiveReserves, getPriceAverage, getPairFees), обмен swap(to, zeroForOne, amountSpecified, data) + обратный вызов
//! smardexSwapCallback(int256, int256, bytes) у вызывающего, события пары; Multicall3.getCurrentBlockTimestamp — время
//! блока чтения в той же пачке. Реестр фабрики (getPair, allPairs, PairCreated) и поля пары factory/token0/token1 — те же
//! подписи, что у Uniswap V2 (abi_v2v3.rs IV2Factory, abi_check.rs): проверка и поиск пар идут путём V2.
//! Кто вызывает: pools/load_smardex.rs, events_smardex.rs.
//! От чего зависит: alloy::sol!.
//! Настройки: нет (фабрика — dex.factory [DEX]).
//!
//! Источник (решение 23): фабрика BSC 0xA8EF6FEa013034E62E2C4A9Ec1CDb059fE23Af33 — Sourcify match (SmardexFactory.sol, solc
//! 0.8.17, в составе — SmardexPair.sol, SmardexLibrary.sol) = официальный репозиторий github.com/SmarDex-Ecosystem/
//! dex-smart-contracts@74d0444 deployments/bsc/SmardexFactory.json (адрес, блок развёртывания 30036610, solcInputs/
//! 1a3e5ed8…; тексты пары, библиотеки и фабрики совпали с Sourcify). contracts/core/interfaces/ISmardexPair.sol — события
//! Mint L41, Burn L51, Swap L62, Sync L73-L80, FeesChanged L87; SmardexPair.sol — getReserves L80-L83, getFictiveReserves
//! L86-L89, getPairFees L98-L101, getPriceAverage L104-L112, swap L155-L319 (вызывающий не ограничен: только lock и «to»
//! не токен пары, L175); ISmardexSwapCallback.sol L21; комиссии — в паре (initialize L72-L77 получает значения фабрики —
//! SmardexFactory.sol createPair L42, setFees L322-L339 — владелец фабрики, событие FeesChanged). Multicall3 — github.com/mds1/multicall3 (getCurrentBlockTimestamp).

// Код функций и событий генерирует alloy::sol! — число аргументов задаёт ABI контракта, а не мы.
#![allow(clippy::too_many_arguments)]

use alloy::sol_types::SolEvent;
use alloy_primitives::B256;

alloy::sol! {
    interface ISmardexPair {
        function getReserves() external view returns (uint256 reserve0_, uint256 reserve1_);
        function getFictiveReserves() external view returns (uint256 fictiveReserve0_, uint256 fictiveReserve1_);
        function getPairFees() external view returns (uint128 feesLP_, uint128 feesPool_);
        function getPriceAverage() external view returns (uint256 priceAverage0_, uint256 priceAverage1_, uint256 priceAverageLastTimestamp_);

        event Mint(address indexed sender, address indexed to, uint256 amount0, uint256 amount1);
        event Burn(address indexed sender, address indexed to, uint256 amount0, uint256 amount1);
        event Swap(address indexed sender, address indexed to, int256 amount0, int256 amount1);
        event Sync(uint256 reserve0, uint256 reserve1, uint256 fictiveReserve0, uint256 fictiveReserve1, uint256 priceAverage0, uint256 priceAverage1);
        event FeesChanged(uint256 indexed feesLP, uint256 indexed feesPool);
    }

    interface IMulticall3Time {
        function getCurrentBlockTimestamp() external view returns (uint256 timestamp);
    }
}

/// topic0 событий изменения ликвидности (Mint, Burn). Ошибок нет.
pub fn liquidity_topics() -> Vec<B256> {
    vec![
        ISmardexPair::Mint::SIGNATURE_HASH,
        ISmardexPair::Burn::SIGNATURE_HASH,
    ]
}

/// topic0 всех событий пары, после которых она перечитывается (Sync, Swap, FeesChanged, ликвидность). Ошибок нет.
pub fn topics() -> Vec<B256> {
    let mut out = vec![
        ISmardexPair::Sync::SIGNATURE_HASH,
        ISmardexPair::Swap::SIGNATURE_HASH,
        ISmardexPair::FeesChanged::SIGNATURE_HASH,
    ];
    out.extend(liquidity_topics());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::sol_types::SolCall;
    use alloy_primitives::keccak256;

    /// Селекторы и topic0 — keccak256 подписей исходника (совпадают с PUSH4 / записанными логами пар BSC).
    #[test]
    fn signatures_match_official() {
        assert_eq!(
            ISmardexPair::getFictiveReservesCall::SELECTOR,
            [0x9a, 0x20, 0x76, 0x7b]
        );
        assert_eq!(
            ISmardexPair::getPriceAverageCall::SELECTOR,
            [0xee, 0x20, 0x46, 0xc7]
        );
        assert_eq!(
            ISmardexPair::getPairFeesCall::SELECTOR,
            [0x00, 0x3b, 0x15, 0xfd]
        );
        assert_eq!(
            IMulticall3Time::getCurrentBlockTimestampCall::SELECTOR,
            [0x0f, 0x28, 0xc9, 0x7d]
        );
        assert_eq!(
            ISmardexPair::Sync::SIGNATURE_HASH,
            B256::from(keccak256(
                b"Sync(uint256,uint256,uint256,uint256,uint256,uint256)"
            ))
        );
    }
}
