//! Файл: venues/bsc_dex/pools/abi_v4.rs
//! Что делает: ABI менеджеров пулов V4 — PancakeSwap Infinity CL и Bin, Uniswap V4 PoolManager и StateView —
//! для eth_call и разбора событий.
//! Кто вызывает: pools/discover_v4.rs, load_cl.rs, load_bin.rs, events_cl.rs, events_bin.rs.
//! От чего зависит: alloy::sol!.
//! Настройки: нет.
//!
//! Сигнатуры — из официальных исходников (research/flow/bsc_v4.md, разделы 3–4):
//! - Infinity CL: https://github.com/pancakeswap/infinity-core/blob/3c5ef1be1f71442d8bc60cc6fd11f9c677b714df/src/pool-cl/interfaces/ICLPoolManager.sol#L37-L108 ;
//!   Tick.Info — src/pool-cl/libraries/Tick.sol#L31-L40.
//! - Infinity Bin: …/src/pool-bin/interfaces/IBinPoolManager.sol#L52-L167.
//! - Общие события Infinity: DynamicLPFeeUpdated — …/src/interfaces/IPoolManager.sol#L22; ProtocolFeeUpdated — …/src/interfaces/IProtocolFees.sol#L23.
//! - Uniswap V4: https://github.com/Uniswap/v4-core/blob/bea7f6d1c11cd5dc4f0d5151ffa37a1c239feee6/src/interfaces/IPoolManager.sol#L60-L100 ,
//!   ProtocolFeeUpdated — src/interfaces/IProtocolFees.sol#L23; StateView —
//!   https://github.com/Uniswap/v4-periphery/blob/0c4e5bd3715ce2e72fd7609a1e2835f8300baf7b/src/lens/StateView.sol#L19-L77 .
//!
//! PoolId, Currency, IHooks в ABI — bytes32 и address (bsc_v4.md, раздел 3).

// Код функций и событий генерирует alloy::sol! — число аргументов задаёт ABI контракта, а не мы.
#![allow(clippy::too_many_arguments)]

alloy::sol! {
    interface IInfinityCl {
        struct TickInfo { uint128 liquidityGross; int128 liquidityNet; uint256 feeGrowthOutside0X128; uint256 feeGrowthOutside1X128; }
        event Initialize(bytes32 indexed id, address indexed currency0, address indexed currency1, address hooks, uint24 fee, bytes32 parameters, uint160 sqrtPriceX96, int24 tick);
        event ModifyLiquidity(bytes32 indexed id, address indexed sender, int24 tickLower, int24 tickUpper, int256 liquidityDelta, bytes32 salt);
        event Swap(bytes32 indexed id, address indexed sender, int128 amount0, int128 amount1, uint160 sqrtPriceX96, uint128 liquidity, int24 tick, uint24 fee, uint16 protocolFee);
        function getSlot0(bytes32 id) external view returns (uint160 sqrtPriceX96, int24 tick, uint24 protocolFee, uint24 lpFee);
        function getLiquidity(bytes32 id) external view returns (uint128 liquidity);
        function getPoolTickInfo(bytes32 id, int24 tick) external view returns (TickInfo memory);
        function getPoolBitmapInfo(bytes32 id, int16 word) external view returns (uint256 tickBitmap);
    }

    interface IInfinityBin {
        event Initialize(bytes32 indexed id, address indexed currency0, address indexed currency1, address hooks, uint24 fee, bytes32 parameters, uint24 activeId);
        event Swap(bytes32 indexed id, address indexed sender, int128 amount0, int128 amount1, uint24 activeId, uint24 fee, uint16 protocolFee);
        event Mint(bytes32 indexed id, address indexed sender, uint256[] ids, bytes32 salt, bytes32[] amounts, bytes32 compositionFeeAmount, bytes32 feeAmountToProtocol);
        event Burn(bytes32 indexed id, address indexed sender, uint256[] ids, bytes32 salt, bytes32[] amounts);
        event Donate(bytes32 indexed id, address indexed sender, int128 amount0, int128 amount1, uint24 binId);
        function getSlot0(bytes32 id) external view returns (uint24 activeId, uint24 protocolFee, uint24 lpFee);
        function getBin(bytes32 id, uint24 binId) external view returns (uint128 binReserveX, uint128 binReserveY, uint256 binLiquidity, uint256 totalShares);
        function getNextNonEmptyBin(bytes32 id, bool swapForY, uint24 binId) external view returns (uint24 nextId);
    }

    interface IInfinityFees {
        event DynamicLPFeeUpdated(bytes32 indexed id, uint24 dynamicLPFee);
        event ProtocolFeeUpdated(bytes32 indexed id, uint24 protocolFee);
    }

    interface IUniswapV4 {
        event Initialize(bytes32 indexed id, address indexed currency0, address indexed currency1, uint24 fee, int24 tickSpacing, address hooks, uint160 sqrtPriceX96, int24 tick);
        event ModifyLiquidity(bytes32 indexed id, address indexed sender, int24 tickLower, int24 tickUpper, int256 liquidityDelta, bytes32 salt);
        event Swap(bytes32 indexed id, address indexed sender, int128 amount0, int128 amount1, uint160 sqrtPriceX96, uint128 liquidity, int24 tick, uint24 fee);
        event ProtocolFeeUpdated(bytes32 indexed id, uint24 protocolFee);
    }

    interface IStateView {
        function getSlot0(bytes32 poolId) external view returns (uint160 sqrtPriceX96, int24 tick, uint24 protocolFee, uint24 lpFee);
        function getLiquidity(bytes32 poolId) external view returns (uint128 liquidity);
        function getTickLiquidity(bytes32 poolId, int24 tick) external view returns (uint128 liquidityGross, int128 liquidityNet);
        function getTickBitmap(bytes32 poolId, int16 tick) external view returns (uint256 tickBitmap);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::sol_types::SolEvent;

    /// topic0 совпадают с посчитанными в research/flow/bsc_v4.md, раздел 3–4 (keccak канонических сигнатур).
    #[test]
    fn topics_match_research() {
        let hex = |b: alloy_primitives::B256| b.to_string();
        assert_eq!(
            hex(IInfinityCl::Initialize::SIGNATURE_HASH),
            "0x426cc62fe6a33a40ba2788c2c87a9c34ee4582b95bc9fa5a7bb7ae70b750b99c"
        );
        assert_eq!(
            hex(IInfinityBin::Initialize::SIGNATURE_HASH),
            "0xddfde5903015c0eb1671976c6c8f760f1328bec57f15286b6bdab2f955cab9c9"
        );
        assert_eq!(
            hex(IUniswapV4::Initialize::SIGNATURE_HASH),
            "0xdd466e674ea557f56295e2d0218a125ea4b4f0f6f3307b95f85e6110838d6438"
        );
        assert_eq!(
            hex(IInfinityCl::Swap::SIGNATURE_HASH),
            "0x04206ad2b7c0f463bff3dd4f33c5735b0f2957a351e4f79763a4fa9e775dd237"
        );
        assert_eq!(
            hex(IInfinityCl::ModifyLiquidity::SIGNATURE_HASH),
            "0xf208f4912782fd25c7f114ca3723a2d5dd6f3bcc3ac8db5af63baa85f711d5ec"
        );
        assert_eq!(
            hex(IInfinityBin::Swap::SIGNATURE_HASH),
            "0x3e8aae37f890eb1f9d63dd4d2062f3f0be757848a0f0760e4f3e53dad556e861"
        );
        assert_eq!(
            hex(IInfinityBin::Mint::SIGNATURE_HASH),
            "0x7b6bc49b385af8644341f07a67cd976bf9daf2bdd5d71668e651a3a792e318e1"
        );
        assert_eq!(
            hex(IInfinityBin::Burn::SIGNATURE_HASH),
            "0x16d40aa4e497175b58e47cbf101544758de8c01d92a760c738943044c758df8a"
        );
        assert_eq!(
            hex(IInfinityFees::DynamicLPFeeUpdated::SIGNATURE_HASH),
            "0x14b2b80e0d62303dc85494859f35a84579160aafbd650180ddf526b1ab547bd6"
        );
        assert_eq!(
            hex(IInfinityFees::ProtocolFeeUpdated::SIGNATURE_HASH),
            "0xe9c42593e71f84403b84352cd168d693e2c9fcd1fdbcc3feb21d92b43e6696f9"
        );
        assert_eq!(
            hex(IUniswapV4::Swap::SIGNATURE_HASH),
            "0x40e9cecb9f5f1f1c5b9c97dec2917b7ee92e57ba5563708daca94dd84ad7112f"
        );
    }
}
