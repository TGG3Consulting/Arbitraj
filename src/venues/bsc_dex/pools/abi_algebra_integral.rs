//! Файл: venues/bsc_dex/pools/abi_algebra_integral.rs
//! Что делает: решение 188 — ABI пула Algebra Integral (THENA V3, DexSwap и другие форки Integral) для чтения
//! состояния (load_algebra_integral.rs) и событий комиссии (events_algebra_integral.rs). Обмен (swap + обратный вызов
//! algebraSwapCallback), события Swap/Mint/Burn и фабрика (poolByPair, событие Pool) у Integral — те же, что у Algebra
//! V1.9 (pools/abi_algebra.rs), их здесь не повторяем.
//! Кто вызывает: pools/load_algebra_integral.rs, pools/events_algebra_integral.rs.
//! От чего зависит: alloy::sol!.
//! Настройки: нет.
//!
//! Сигнатуры — официальный код cryptoalgebra/Algebra, пакет @cryptoalgebra/integral-core 1.2.10 (коммит 1ff43e1):
//! - IAlgebraPoolState.sol: globalState() → (uint160 price, int24 tick, uint16 lastFee, uint8 pluginConfig,
//!   uint16 communityFee, bool unlocked) (L44); ticks(int24) → (uint256 liquidityTotal, int128 liquidityDelta,
//!   int24 prevTick, int24 nextTick, uint256 outerFeeGrowth0Token, uint256 outerFeeGrowth1Token) (L57-L69) — тики
//!   связаны СПИСКОМ (prevTick/nextTick; концы — TickMath.MIN_TICK и MAX_TICK, TickManagement.initTickState L131-L132);
//!   fee() → uint16 (L117: при флаге DYNAMIC_FEE — getCurrentFee() плагина, AlgebraPoolBase.sol L145-L150);
//!   liquidity() → uint128 (L143); prevTickGlobal()/nextTickGlobal() → int24 (L156, L161) — ближайшие
//!   инициализированные тики слева (≤ текущего) и справа (> текущего).
//! - IAlgebraPoolEvents.sol: Fee(uint16 fee) (L122) — новая lastFee; SwapFee(address indexed sender, uint24
//!   overrideFee, uint24 pluginFee) (L86) — комиссия, с которой прошёл обмен (SwapCalculation.sol: overrideFee ≠ 0 →
//!   fee = overrideFee + pluginFee, иначе lastFee + pluginFee).
//! - Plugins.sol L26: DYNAMIC_FEE = 1 << 7 (флаг в globalState.pluginConfig).

alloy::sol! {
    #[allow(missing_docs)]
    interface IAlgebraIntegralPool {
        event Fee(uint16 fee);
        event SwapFee(address indexed sender, uint24 overrideFee, uint24 pluginFee);
        function globalState() external view returns (uint160 price, int24 tick, uint16 lastFee, uint8 pluginConfig, uint16 communityFee, bool unlocked);
        function liquidity() external view returns (uint128);
        function fee() external view returns (uint16 currentFee);
        function prevTickGlobal() external view returns (int24);
        function nextTickGlobal() external view returns (int24);
        function ticks(int24 tick) external view returns (uint256 liquidityTotal, int128 liquidityDelta, int24 prevTick, int24 nextTick, uint256 outerFeeGrowth0Token, uint256 outerFeeGrowth1Token);
    }
}

/// Флаг динамической комиссии в globalState.pluginConfig (Plugins.sol L26).
pub const DYNAMIC_FEE: u8 = 1 << 7;
