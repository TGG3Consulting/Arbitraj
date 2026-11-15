//! Файл: venues/bsc_dex/pools/abi_nomi.rs
//! Что делает: ABI пары Nomiswap Stable (решение 284, DEX nomiswap_stable): геттеры состояния (load_nomi.rs), квотер —
//! сама пара getAmountOut(address tokenIn, uint256 amountIn) (селектор 0xca706bcf — порядок аргументов обратный
//! Solidly getAmountOut(uint256,address)), topic0 событий пары (events_nomi.rs). Реестр фабрики (getPair,
//! PairCreated), поля пары factory/token0/token1, события Sync/Swap/Mint/Burn — те же подписи, что у Uniswap V2
//! (abi_v2v3.rs IV2Factory / IV2Pair): проверка и поиск пар идут путём V2 (verify.rs, factory_calls.rs, created_log.rs).
//! Кто вызывает: pools/load_nomi.rs, pools/events_nomi.rs, quote/quoter_calls.rs.
//! От чего зависит: alloy::sol!.
//! Настройки: нет (фабрика — dex.factory [DEX]).
//!
//! Источник (решение 23): verified-исходник Sourcify full match, chain 56 — пары 0x1e40450f8e21bb68490d7d91ab422888fb3d60f1
//! (USDT/USDC), 0x3a667100753cfb7538208af98cb472f65f10da87 (USDT/BUSD), 0xca25f030a103bcd0a6360ffe0f98d95f3cc6a7ec
//! (USDC/BUSD) — один и тот же NomiswapStablePair.sol (solc 0.8.15): swapFee L47 (uint32, над MAX_FEE = 100000, L20),
//! swap L132-L176, getAmountOut L251-L274, getReserves L276-L280, getA L286-L303 (рампа A по block.timestamp),
//! _update L313-L320 (emit Sync); token0PrecisionMultiplier / token1PrecisionMultiplier L40-L41 (public immutable);
//! события — interfaces/INomiswapPair.sol L8-L18 (Mint, Burn, Swap, Sync(uint112, uint112) — подписи как у V2),
//! interfaces/INomiswapStablePair.sol L9-L10 (RampA, StopRampA). setSwapFee (L71-L74) события НЕ шлёт — комиссия и рампа
//! A меняются без события: вид перечитывается каждым блоком (per_block.rs). Фабрика 0xc6b7ee49d386bae4fd501f2d2f8d18828f1f6285
//! (Sourcify full match, NomiswapStableFactory; allPairsLength = 3 на блоке 133650877, tests/data/nomiswap_stable_2026-11-15):
//! getPair / allPairs / PairCreated(address indexed, address indexed, address, uint) — как UniswapV2Factory.

// Код функций и событий генерирует alloy::sol! — число аргументов задаёт ABI контракта, а не мы.
#![allow(clippy::too_many_arguments)]

use alloy::sol_types::SolEvent;
use alloy_primitives::B256;

alloy::sol! {
    interface INomiStablePair {
        function getReserves() external view returns (uint112 reserve0, uint112 reserve1, uint32 blockTimestampLast);
        function swapFee() external view returns (uint32);
        function getA() external view returns (uint256);
        function token0PrecisionMultiplier() external view returns (uint128);
        function token1PrecisionMultiplier() external view returns (uint128);
        function getAmountOut(address tokenIn, uint256 amountIn) external view returns (uint256 finalAmountOut);

        event Mint(address indexed sender, uint256 amount0, uint256 amount1);
        event Burn(address indexed sender, uint256 amount0, uint256 amount1, address indexed to);
        event Swap(address indexed sender, uint256 amount0In, uint256 amount1In, uint256 amount0Out, uint256 amount1Out, address indexed to);
        event Sync(uint112 reserve0, uint112 reserve1);
        event RampA(uint256 oldA, uint256 newA, uint256 initialTime, uint256 futureTime);
        event StopRampA(uint256 A, uint256 t);
    }
}

/// Делитель комиссии пары MAX_FEE = 100000 (NomiswapStablePair.sol L20: 100 %) — константа протокола.
pub const MAX_FEE: u64 = 100_000;
/// A_PRECISION = 100 (L21) — getA() возвращает A·100.
pub const A_PRECISION: u64 = 100;
/// MAX_LOOP_LIMIT = 256 (L28) — предел раундов D и y; несходимость не откатывает (цикл кончается).
pub const MAX_ROUNDS: u16 = 256;

/// topic0 событий изменения ликвидности (Mint, Burn — как у V2; мелкие пулы слушают только их, depth.rs). Ошибок нет.
pub fn liquidity_topics() -> Vec<B256> {
    use INomiStablePair as P;
    vec![P::Mint::SIGNATURE_HASH, P::Burn::SIGNATURE_HASH]
}

/// topic0 всех событий пары, после которых она перечитывается (Sync, Swap, ликвидность, рампа A). Ошибок нет.
pub fn topics() -> Vec<B256> {
    use INomiStablePair as P;
    let mut out = vec![
        P::Sync::SIGNATURE_HASH,
        P::Swap::SIGNATURE_HASH,
        P::RampA::SIGNATURE_HASH,
        P::StopRampA::SIGNATURE_HASH,
    ];
    out.extend(liquidity_topics());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::sol_types::SolCall;
    use alloy_primitives::keccak256;

    /// Селекторы и topic0 — keccak256 подписей из verified-исходника; Sync/Swap — те же, что у V2.
    #[test]
    fn signatures_match_official() {
        let sel = |s: &str| keccak256(s.as_bytes())[..4].to_vec();
        assert_eq!(
            INomiStablePair::getAmountOutCall::SELECTOR.to_vec(),
            vec![0xca, 0x70, 0x6b, 0xcf]
        );
        assert_eq!(INomiStablePair::getACall::SELECTOR.to_vec(), sel("getA()"));
        assert_eq!(
            INomiStablePair::RampA::SIGNATURE_HASH,
            B256::from(keccak256(b"RampA(uint256,uint256,uint256,uint256)"))
        );
        use super::super::abi_v2v3::IV2Pair;
        assert_eq!(
            INomiStablePair::Sync::SIGNATURE_HASH,
            IV2Pair::Sync::SIGNATURE_HASH
        );
        assert_eq!(
            INomiStablePair::Swap::SIGNATURE_HASH,
            IV2Pair::Swap::SIGNATURE_HASH
        );
        assert_eq!(
            INomiStablePair::Mint::SIGNATURE_HASH,
            IV2Pair::Mint::SIGNATURE_HASH
        );
    }
}
