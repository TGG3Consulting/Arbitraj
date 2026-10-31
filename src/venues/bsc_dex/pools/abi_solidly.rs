//! Файл: venues/bsc_dex/pools/abi_solidly.rs
//! Что делает: ABI пула и фабрики семейства Solidly (ve(3,3)-форки: Topaz v2, THENA V1; решения 80, 85) — для
//! eth_call чтения состояния (load_solidly.rs), проверки пула через фабрику (verify_solidly.rs), разбора события
//! Sync (events_solidly.rs), сверки с квотером (quote/quoter_calls.rs — сам пул getAmountOut) и разбора события
//! создания пула (created_log.rs).
//! Кто вызывает: pools/load_solidly.rs, pools/events_solidly.rs, pools/verify_solidly.rs, pools/created_log.rs,
//! quote/quoter_calls.rs.
//! От чего зависит: alloy::sol!.
//! Настройки: нет (адрес фабрики — dex.factory по виду пула; адрес пула — из справочника).
//!
//! Сигнатуры — из официального публичного Velodrome (velodrome-finance/contracts, HEAD
//! b3065d8b6702b14b094f9f6046b752cc9f78c43b, решение 23; тот же HEAD, что math/swap_solidly.rs). Это каноническая
//! форма Solidly-пула, по которой котируют его сами DEX; THENA V1 и Topaz — форки этой же модели (у пула есть флаг
//! `stable`, в пуле getAmountOut, фабрика getPool/isPool). Разделы:
//! - пул: metadata() → (dec0, dec1, reserve0, reserve1, stable, token0, token1) — Pool.sol L122-L129 (dec0/dec1 —
//!   это 10^decimals токена, Pool.sol L93-L94); getReserves() → (reserve0, reserve1, blockTimestampLast) L204-L208;
//!   stable()/token0()/token1() — IPool.sol; getAmountOut(amountIn, tokenIn) — Pool.sol L447-L451 (квотер, решение 73);
//!   событие Sync(uint256 reserve0, uint256 reserve1) — IPool.sol L29 (у Solidly резервы uint256, НЕ uint112 как у
//!   UniswapV2 — поэтому семейство отдельное, обычный V2-форк его не покрывает).
//! - фабрика: getPool(tokenA, tokenB, bool stable) → address (реестр, заполнен в обе стороны token-ов — PoolFactory.sol
//!   L143-L156); isPool(address) → bool (IPoolFactory.sol L36); getFee(address pool, bool stable) → uint256 (комиссия
//!   пула, числитель над 10000 — PoolFactory.sol L131-L133); событие PoolCreated(token0, token1, stable, pool, uint)
//!   (IPoolFactory.sol L9).

alloy::sol! {
    interface ISolidlyPool {
        event Sync(uint256 reserve0, uint256 reserve1);

        function metadata() external view returns (uint256 dec0, uint256 dec1, uint256 r0, uint256 r1, bool st, address t0, address t1);
        function getReserves() external view returns (uint256 reserve0, uint256 reserve1, uint256 blockTimestampLast);
        function stable() external view returns (bool);
        function token0() external view returns (address);
        function token1() external view returns (address);
        // Квотер семейства Solidly — сам пул (решение 73): выход за amountIn с учётом комиссии пула.
        function getAmountOut(uint256 amountIn, address tokenIn) external view returns (uint256);
    }

    interface ISolidlyFactory {
        event PoolCreated(address indexed token0, address indexed token1, bool indexed stable, address pool, uint256 allPoolsLength);

        function getPool(address tokenA, address tokenB, bool stable) external view returns (address);
        function isPool(address pool) external view returns (bool);
        function getFee(address pool, bool stable) external view returns (uint256);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::sol_types::{SolCall, SolEvent};
    use alloy_primitives::{B256, keccak256};

    /// topic0 и селекторы совпадают с keccak256 канонических сигнатур официального контракта (velodrome-finance).
    #[test]
    fn signatures_match_official() {
        let topic = |sig: &str| B256::from(keccak256(sig.as_bytes()));
        assert_eq!(
            ISolidlyPool::Sync::SIGNATURE_HASH,
            topic("Sync(uint256,uint256)")
        );
        assert_eq!(
            ISolidlyFactory::PoolCreated::SIGNATURE_HASH,
            topic("PoolCreated(address,address,bool,address,uint256)")
        );
        let sel = |sig: &str| keccak256(sig.as_bytes())[..4].to_vec();
        assert_eq!(
            ISolidlyPool::metadataCall::SELECTOR.to_vec(),
            sel("metadata()")
        );
        assert_eq!(
            ISolidlyPool::getReservesCall::SELECTOR.to_vec(),
            sel("getReserves()")
        );
        assert_eq!(
            ISolidlyPool::getAmountOutCall::SELECTOR.to_vec(),
            sel("getAmountOut(uint256,address)")
        );
        assert_eq!(
            ISolidlyFactory::getPoolCall::SELECTOR.to_vec(),
            sel("getPool(address,address,bool)")
        );
        assert_eq!(
            ISolidlyFactory::isPoolCall::SELECTOR.to_vec(),
            sel("isPool(address)")
        );
        assert_eq!(
            ISolidlyFactory::getFeeCall::SELECTOR.to_vec(),
            sel("getFee(address,bool)")
        );
    }
}
