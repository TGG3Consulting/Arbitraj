//! Файл: venues/bsc_dex/pools/abi_algebra_dir.rs
//! Что делает: решение 444 — ABI пула Algebra V1.9 с направленными комиссиями (Camelot V3, вид algebra_dir_fee): то,
//! чем он отличается от Algebra V1.9 THENA Fusion (pools/abi_algebra.rs) — globalState с двумя комиссиями и событие
//! Fee с двумя комиссиями. Остальное (Swap/Mint/Burn, liquidity, ticks, tickTable, tickSpacing, фабрика poolByPair и
//! событие Pool) — то же, что у V1.9, и берётся из abi_algebra.rs.
//! Кто вызывает: pools/load_algebra_dir.rs, pools/events_algebra_dir.rs, pools/prefetch_algebra_dir.rs, pools/settle.rs.
//! От чего зависит: alloy::sol!.
//! Настройки: нет.
//!
//! Источник — cryptoalgebra/AlgebraV1.9, ветка direction-based-fee, коммит a45417ed5048c0a9ad3c673e41a00bb3c3880f6f
//! (официальная версия «V1.9 — directional fees», docs.algebra.finance/algebra-integral-documentation/
//! algebra-v1-technical-reference/contracts/api-reference-v1.9-directional-fees; npm
//! @cryptoalgebra/v1.9-directional-fee-core 1.0.0 — README.md ветки):
//! - src/core/contracts/interfaces/pool/IAlgebraPoolState.sol:22-33 — globalState() → (uint160 price, int24 tick,
//!   uint16 feeZto, uint16 feeOtz, uint16 timepointIndex, uint8 communityFeeToken0, uint8 communityFeeToken1,
//!   bool unlocked); хранилище — src/core/contracts/base/PoolState.sol:8-17;
//! - src/core/contracts/interfaces/pool/IAlgebraPoolEvents.sol:107 — event Fee(uint16 feeZto, uint16 feeOtz);
//!   испускает AlgebraPool.sol:558 (_updateFee) при каждой смене комиссий.
//!
//! Разница с main (779b5681, = THENA Fusion) по ядру — только комиссии: git diff main..a45417ed по
//! src/core/contracts/AlgebraPool.sol, PoolState.sol, IAlgebraPoolEvents.sol, IAlgebraPoolState.sol (globalState,
//! _updateFee, выбор комиссии по направлению в _calculateSwapAndLock, Fee, комиссия flash = BASE_FEE).

// Код функций и событий генерирует alloy::sol! — число аргументов и полей задаёт ABI контракта, а не мы.
#![allow(clippy::too_many_arguments)]

alloy::sol! {
    interface IAlgebraDirPool {
        event Fee(uint16 feeZto, uint16 feeOtz);
        function globalState() external view returns (uint160 price, int24 tick, uint16 feeZto, uint16 feeOtz, uint16 timepointIndex, uint8 communityFeeToken0, uint8 communityFeeToken1, bool unlocked);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::sol_types::{SolCall, SolEvent};
    use alloy_primitives::{B256, keccak256};

    /// topic0 и селектор совпадают с keccak256 канонических сигнатур (IAlgebraPoolEvents.sol:107,
    /// IAlgebraPoolState.sol:22); селектор globalState() — тот же, что у V1.9 без направлений (ответ — другой).
    #[test]
    fn signatures_match_official() {
        assert_eq!(
            IAlgebraDirPool::Fee::SIGNATURE_HASH,
            B256::from(keccak256(b"Fee(uint16,uint16)"))
        );
        assert_eq!(
            IAlgebraDirPool::globalStateCall::SELECTOR.to_vec(),
            keccak256(b"globalState()")[..4].to_vec()
        );
        assert_ne!(
            IAlgebraDirPool::Fee::SIGNATURE_HASH,
            super::super::abi_algebra_integral::IAlgebraIntegralPool::Fee::SIGNATURE_HASH
        );
    }
}
