//! Файл: venues/bsc_dex/pools/abi_stable_u.rs
//! Что делает: ABI пулов StableSwap на Solidity с индексами uint256 (решение 122 F, 3) — PancakeSwap StableSwap и Lista
//! StableSwap: обмен и квотер `exchange/get_dy(uint256, …)`, флаги остановки (is_killed у PancakeSwap, paused у Lista),
//! геттеры A с точностью у Lista (initial_A/future_A/…), проверка цены Lista (oracle, пороги, skipPriceDiff,
//! fetchOraclePrice, admin_fee), реестры фабрик (PancakeSwap getPairInfo / getThreePoolPairInfo, Lista getPairInfos) и
//! topic0 событий пулов. Геттеры coins(uint256)/balances(uint256)/A()/fee() — те же селекторы, что у Ellipsis
//! (abi_curve.rs IStableSwapV1) — читаются им.
//! Кто вызывает: pools/load_stable_u.rs, pools/verify_stable_u.rs, pools/events_stable_u.rs, pools/factory_ask.rs,
//! quote/quoter_calls.rs.
//! От чего зависит: alloy::sol!, alloy-primitives.
//! Настройки: нет.
//!
//! Источники (официальные, решение 23):
//! • PancakeSwap: pancake-smart-contracts cb079908 (копия — scratchpad/sources/pancake-stable/github-fork-migo557),
//!   projects/stable-swap/contracts/PancakeStableSwapTwoPool.sol — события L59-L95, `is_killed` L54, `get_dy` L393,
//!   `exchange(uint256,uint256,uint256,uint256) payable` L425 (ThreePool — те же строки); PancakeStableSwapFactory.sol —
//!   `getPairInfo` L194, `getThreePoolPairInfo` L203, событие `NewStableSwapPair(address indexed,address,address,
//!   address,address)` L38; те же
//!   сигнатуры — официальный сабграф pancakeswap/pancake-subgraph subgraphs/exchange-stableswap/bsc/subgraph.yaml и
//!   pancakeswap/infinity-universal-router src/interfaces/IStableSwap.sol, IStableSwapFactory.sol.
//! • Lista: lista-dao/moolah eb7d73a, src/dex/StableSwapPool.sol — A_PRECISION L41, admin_fee L48, initial_A … L58-L61,
//!   oracle/пороги/skipPriceDiff L67-L74, `fetchOraclePrice` L424, `exchange` L478 (whenNotPaused);
//!   src/dex/interfaces/IStableSwap.sol — события L63-L112; src/dex/StableSwapFactory.sol — `getPairInfos` L206,
//!   событие `NewStableSwapPair(address indexed,address,address,address)` L32.

use alloy_primitives::{B256, keccak256};

alloy::sol! {
    /// Пул PancakeSwap StableSwap (Two/ThreePool): обмен, квотер, флаг остановки, события.
    interface IPancakeStable {
        event TokenExchange(address indexed buyer, uint256 sold_id, uint256 tokens_sold, uint256 bought_id, uint256 tokens_bought);
        event RemoveLiquidityOne(address indexed provider, uint256 index, uint256 token_amount, uint256 coin_amount);
        event NewFee(uint256 fee, uint256 admin_fee);
        event RampA(uint256 old_A, uint256 new_A, uint256 initial_time, uint256 future_time);
        event StopRampA(uint256 A, uint256 t);
        event Kill();
        event Unkill();

        function is_killed() external view returns (bool);
        function get_dy(uint256 i, uint256 j, uint256 dx) external view returns (uint256);
        function exchange(uint256 i, uint256 j, uint256 dx, uint256 min_dy) external payable;
    }

    /// Фабрика PancakeSwap StableSwap V2: реестр пар (двух- и трёхмонетных пулов).
    interface IPancakeStableFactory {
        struct StableSwapPairInfo { address swapContract; address token0; address token1; address LPContract; }
        struct StableSwapThreePoolPairInfo { address swapContract; address token0; address token1; address token2; address LPContract; }
        function getPairInfo(address _tokenA, address _tokenB) external view returns (StableSwapPairInfo memory info);
        function getThreePoolPairInfo(address _tokenA, address _tokenB) external view returns (StableSwapThreePoolPairInfo memory info);
    }

    /// Пул Lista StableSwap: A с точностью, проверка цены против оракула, пауза, события.
    interface IListaStable {
        event TokenExchange(address indexed buyer, uint256 sold_id, uint256 tokens_sold, uint256 bought_id, uint256 tokens_bought, uint256 swap_fee, uint256 admin_fee);
        event RemoveLiquidityOne(address indexed provider, uint256 index, uint256 token_amount, uint256 coin_amount, uint256 fee, uint256 admin_fee_rate);
        event NewFee(uint256 fee, uint256 admin_fee);
        event RampA(uint256 old_A, uint256 new_A, uint256 initial_time, uint256 future_time);
        event StopRampA(uint256 A, uint256 t);
        event ChangePriceDiffThreshold(uint256 price0DiffThreshold, uint256 price1DiffThreshold);
        event SetSkipPriceDiff(bool skipPriceDiff);
        event ChangeOracle(address newOracle);
        event Paused(address account);
        event Unpaused(address account);

        function initial_A() external view returns (uint256);
        function future_A() external view returns (uint256);
        function initial_A_time() external view returns (uint256);
        function future_A_time() external view returns (uint256);
        function admin_fee() external view returns (uint256);
        function price0DiffThreshold() external view returns (uint256);
        function price1DiffThreshold() external view returns (uint256);
        function skipPriceDiff() external view returns (bool);
        function fetchOraclePrice() external view returns (uint256[2] memory oraclePrices);
        function paused() external view returns (bool);
        function get_dy(uint256 i, uint256 j, uint256 dx) external view returns (uint256);
        function exchange(uint256 i, uint256 j, uint256 dx, uint256 min_dy) external payable;
    }

    /// Фабрика Lista StableSwap: реестр пар.
    interface IListaStableFactory {
        struct StableSwapPairInfo { address swapContract; address token0; address token1; address LPContract; }
        function getPairInfos(address _tokenA, address _tokenB) external view returns (StableSwapPairInfo[] memory);
    }

    /// Multicall3.getCurrentBlockTimestamp (mds1/multicall3 src/Multicall3.sol) — время блока для рампы A у Lista.
    interface IMulticall3Time {
        function getCurrentBlockTimestamp() external view returns (uint256 timestamp);
    }
}

/// A_PRECISION Lista (StableSwapPool.sol L41). Ошибок нет.
pub const LISTA_A_PRECISION: u64 = 100;

/// topic0 событий ликвидности пула с массивами `uint256[n]`. Принимает: n, true — Lista (у Add/Imbalance лишнее поле
/// admin_fee_rate, IStableSwap.sol L73-L102), false — PancakeSwap (как Curve, сабграф exchange-stableswap/bsc).
/// Возвращает: список topic0. Ошибок нет.
pub fn liquidity_topics(n: usize, lista: bool) -> Vec<B256> {
    let h = |s: String| B256::from(keccak256(s.as_bytes()));
    let tail = if lista { ",uint256" } else { "" };
    vec![
        h(format!(
            "AddLiquidity(address,uint256[{n}],uint256[{n}],uint256,uint256{tail})"
        )),
        h(format!(
            "RemoveLiquidity(address,uint256[{n}],uint256[{n}],uint256)"
        )),
        h(format!(
            "RemoveLiquidityImbalance(address,uint256[{n}],uint256[{n}],uint256,uint256{tail})"
        )),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::sol_types::{SolCall, SolEvent};

    /// Селекторы и topic0 совпадают с keccak256 сигнатур официальных исходников.
    #[test]
    fn signatures_match_official() {
        let sel = |sig: &str| keccak256(sig.as_bytes())[..4].to_vec();
        let topic = |sig: &str| B256::from(keccak256(sig.as_bytes()));
        assert_eq!(
            IPancakeStable::exchangeCall::SELECTOR.to_vec(),
            sel("exchange(uint256,uint256,uint256,uint256)")
        );
        assert_eq!(
            IPancakeStable::exchangeCall::SELECTOR.to_vec(),
            vec![0x5b, 0x41, 0xb9, 0x08]
        );
        assert_eq!(
            IPancakeStable::get_dyCall::SELECTOR.to_vec(),
            sel("get_dy(uint256,uint256,uint256)")
        );
        assert_eq!(
            IPancakeStable::TokenExchange::SIGNATURE_HASH,
            topic("TokenExchange(address,uint256,uint256,uint256,uint256)")
        );
        assert_eq!(
            IListaStable::TokenExchange::SIGNATURE_HASH,
            topic("TokenExchange(address,uint256,uint256,uint256,uint256,uint256,uint256)")
        );
        // topic0 Lista TokenExchange — 0x143f1f8e…f29c (scratchpad/sources/lista/README.md).
        assert!(
            format!("{:x}", IListaStable::TokenExchange::SIGNATURE_HASH).starts_with("143f1f8e")
        );
        assert_eq!(
            IPancakeStableFactory::getPairInfoCall::SELECTOR.to_vec(),
            sel("getPairInfo(address,address)")
        );
        assert_eq!(
            IListaStableFactory::getPairInfosCall::SELECTOR.to_vec(),
            sel("getPairInfos(address,address)")
        );
    }
}
