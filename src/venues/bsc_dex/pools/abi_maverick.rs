//! Файл: venues/bsc_dex/pools/abi_maverick.rs
//! Что делает: ABI Maverick V2 (решение 122 F): пул (чтение состояния — load_maverick.rs; проверка — verify_maverick.rs;
//! события — events_maverick.rs), фабрика (реестр isFactoryPool и поиск по паре lookup — verify_maverick.rs,
//! factory_ask.rs; событие создания PoolCreated — created_log.rs), PoolLens (тики около активного одним вызовом) и
//! Quoter (calculateSwap — сверка порта расчёта перед сделкой и в цикле, решение 73). Только объявления — без сети.
//! Кто вызывает: pools/{load_maverick, verify_maverick, events_maverick, factory_ask, created_log}.rs,
//! quote/quoter_calls.rs.
//! От чего зависит: alloy::sol!.
//! Настройки: нет (адреса — dex.factory, dex.quoter, dex.pool_lens [DEX]).
//!
//! Источник (официальный, решение 23; исходник пула v2-core закрыт — 404):
//! - maverickprotocol/v2-common HEAD d91e8714a20b65913166d6b6608590ff822c3f06, contracts/interfaces/IMaverickV2Pool.sol:
//!   события L30-L53 (PoolSwap, PoolAddLiquidity, PoolMigrateBinsUpStack, PoolRemoveLiquidity, PoolSetVariableFee),
//!   TickState L58-L63, SwapParams L106-L111, AddLiquidityParams L119-L123, RemoveLiquidityParams L130-L133, State
//!   L155-L165, permissionedSwap L206, fee(bool) L212 (D18), tickSpacing L217, tokenA/tokenB L233-L238 (tokenA < tokenB),
//!   factory L244, tokenAScale/tokenBScale L252-L260, getState L290, getTick L300;
//! - contracts/interfaces/IMaverickV2Factory.sol: PoolCreated L35-L47, isFactoryPool L287,
//!   lookup(tokenA, tokenB, startIndex, endIndex) L352-L357;
//! - maverickprotocol/v2-interfaces HEAD f9f4f3ebeec6374a8749a882a05947195e4dc52f: IMaverickV2PoolLens.sol
//!   getTicksAroundActive L263-L266; IMaverickV2Quoter.sol calculateSwap L25-L31 (не view: пул исполняет обмен и
//!   откатывается — исходник квотера MaverickV2Quoter.sol, проверенный в эксплорере Arbitrum по адресу
//!   0xb40AfdB85a07f37aE217E7D6462e609900dD8D7A, L2114-L2134, L2151-L2153; адрес на BSC тот же — docs.mav.xyz
//!   v2-contract-addresses).

// Код функций и событий генерирует alloy::sol! — число аргументов задаёт ABI контракта, а не мы.
#![allow(clippy::too_many_arguments)]

alloy::sol! {
    interface IMaverickV2Pool {
        struct TickState { uint128 reserveA; uint128 reserveB; uint128 totalSupply; uint32[4] binIdsByTick; }
        struct SwapParams { uint256 amount; bool tokenAIn; bool exactOutput; int32 tickLimit; }
        struct AddLiquidityParams { uint8 kind; int32[] ticks; uint128[] amounts; }
        struct RemoveLiquidityParams { uint32[] binIds; uint128[] amounts; }
        struct State {
            uint128 reserveA; uint128 reserveB; int64 lastTwaD8; int64 lastLogPriceD8; uint40 lastTimestamp;
            int32 activeTick; bool isLocked; uint32 binCounter; uint8 protocolFeeRatioD3;
        }

        event PoolSwap(address sender, address recipient, SwapParams params, uint256 amountIn, uint256 amountOut);
        event PoolAddLiquidity(address sender, address recipient, uint256 subaccount, AddLiquidityParams params, uint256 tokenAAmount, uint256 tokenBAmount, uint32[] binIds);
        event PoolMigrateBinsUpStack(address sender, uint32 binId, uint32 maxRecursion);
        event PoolRemoveLiquidity(address sender, address recipient, uint256 subaccount, RemoveLiquidityParams params, uint256 tokenAOut, uint256 tokenBOut);
        event PoolSetVariableFee(uint256 newFeeAIn, uint256 newFeeBIn);

        function permissionedSwap() external view returns (bool);
        function fee(bool tokenAIn) external view returns (uint256);
        function tickSpacing() external view returns (uint256);
        function tokenA() external view returns (address);
        function tokenB() external view returns (address);
        function factory() external view returns (address);
        function tokenAScale() external view returns (uint256);
        function tokenBScale() external view returns (uint256);
        function getState() external view returns (State memory);
        function getTick(int32 tick) external view returns (TickState memory tickState);
    }

    interface IMaverickV2Factory {
        event PoolCreated(address poolAddress, uint8 protocolFeeRatio, uint256 feeAIn, uint256 feeBIn, uint256 tickSpacing, uint256 lookback, int32 activeTick, address tokenA, address tokenB, uint8 kinds, address accessor);

        function isFactoryPool(address pool) external view returns (bool);
        function lookup(address _tokenA, address _tokenB, uint256 startIndex, uint256 endIndex) external view returns (address[] memory pools);
    }

    interface IMaverickV2PoolLens {
        function getTicksAroundActive(address pool, int32 tickRadius) external view returns (int32[] memory ticks, IMaverickV2Pool.TickState[] memory tickStates);
    }

    interface IMaverickV2Quoter {
        function calculateSwap(address pool, uint128 amount, bool tokenAIn, bool exactOutput, int32 tickLimit) external returns (uint256 amountIn, uint256 amountOut, uint256 gasEstimate);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::sol_types::{SolCall, SolEvent};
    use alloy_primitives::{B256, keccak256};

    fn topic(sig: &str) -> B256 {
        keccak256(sig.as_bytes())
    }
    fn sel(sig: &str) -> [u8; 4] {
        keccak256(sig.as_bytes())[..4].try_into().unwrap()
    }

    /// topic0 событий и селекторы — keccak256 канонических сигнатур официальных интерфейсов (кортежи структур).
    #[test]
    fn signatures_match_official() {
        use IMaverickV2Pool as P;
        assert_eq!(
            P::PoolSwap::SIGNATURE_HASH,
            topic("PoolSwap(address,address,(uint256,bool,bool,int32),uint256,uint256)")
        );
        assert_eq!(
            P::PoolAddLiquidity::SIGNATURE_HASH,
            topic(
                "PoolAddLiquidity(address,address,uint256,(uint8,int32[],uint128[]),uint256,uint256,uint32[])"
            )
        );
        assert_eq!(
            P::PoolRemoveLiquidity::SIGNATURE_HASH,
            topic(
                "PoolRemoveLiquidity(address,address,uint256,(uint32[],uint128[]),uint256,uint256)"
            )
        );
        assert_eq!(
            P::PoolMigrateBinsUpStack::SIGNATURE_HASH,
            topic("PoolMigrateBinsUpStack(address,uint32,uint32)")
        );
        assert_eq!(
            P::PoolSetVariableFee::SIGNATURE_HASH,
            topic("PoolSetVariableFee(uint256,uint256)")
        );
        assert_eq!(P::getStateCall::SELECTOR, sel("getState()"));
        assert_eq!(P::feeCall::SELECTOR, sel("fee(bool)"));
        assert_eq!(P::getTickCall::SELECTOR, sel("getTick(int32)"));
        assert_eq!(
            IMaverickV2Factory::PoolCreated::SIGNATURE_HASH,
            topic(
                "PoolCreated(address,uint8,uint256,uint256,uint256,uint256,int32,address,address,uint8,address)"
            )
        );
        assert_eq!(
            IMaverickV2Factory::lookupCall::SELECTOR,
            sel("lookup(address,address,uint256,uint256)")
        );
        assert_eq!(
            IMaverickV2Factory::isFactoryPoolCall::SELECTOR,
            sel("isFactoryPool(address)")
        );
        assert_eq!(
            IMaverickV2PoolLens::getTicksAroundActiveCall::SELECTOR,
            sel("getTicksAroundActive(address,int32)")
        );
        assert_eq!(
            IMaverickV2Quoter::calculateSwapCall::SELECTOR,
            sel("calculateSwap(address,uint128,bool,bool,int32)")
        );
    }
}
