//! Файл: venues/bsc_dex/pools/abi_camelot.rs
//! Что делает: решение 443 (T433-F4) — ABI пары Camelot V2 (CamelotPair, Arbitrum One): геттеры состояния
//! (load_camelot.rs), квотер — сама пара getAmountOut(uint256 amountIn, address tokenIn) (подпись и селектор те же, что у
//! Solidly — quote/quoter_calls.rs берёт ISolidlyPool::getAmountOut), topic0 событий пары (events_camelot.rs). Реестр
//! фабрики (getPair, PairCreated), поля пары factory/token0/token1, события Sync / Mint / Swap / Burn и обмен
//! swap(uint256, uint256, address, bytes) — те же подписи, что у Uniswap V2 (abi_v2v3.rs IV2Factory / IV2Pair):
//! проверка и поиск пар идут путём V2 (verify.rs, factory_calls.rs, created_log.rs).
//! Кто вызывает: pools/load_camelot.rs, pools/events_camelot.rs.
//! От чего зависит: alloy::sol!.
//! Настройки: нет (фабрика — dex.factory [DEX]).
//!
//! Источник: verified-исходник Arbiscan официальной фабрики 0x6EcCab422D763aC031210895C81787E87B43A652
//! (docs.camelot.exchange/contracts/arbitrum/one-mainnet, «AMMv2 Factory»), CamelotFactory.sol L19-L23 (getPair,
//! allPairs, PairCreated(address indexed, address indexed, address, uint)), L54-L70 (createPair: токены по адресу,
//! create2 с кодом CamelotPair); текст пары — тот же, что у verified-исходника пары
//! 0x1c31fb3359357f6436565ccb3e982bc6bf4189ae (копия: tintinweb/smart-contract-sanctuary-arbitrum@465b149,
//! contracts/mainnet/1c/1c31fb3359357f6436565ccb3e982bc6bf4189ae_CamelotPair.sol — «Pair.sol»): getReserves L46-L51
//! (uint112, uint112, uint16, uint16), precisionMultiplier0/1 L30-L31 (10^decimals, L86-L87), stableSwap L35, события
//! FeePercentUpdated L59, SetStableSwap L60, Mint L62, Burn L63, Swap L64-L71, Sync L72; getAmountOut L372.

// Код функций и событий генерирует alloy::sol! — число аргументов задаёт ABI контракта, а не мы.
#![allow(clippy::too_many_arguments)]

use alloy::sol_types::SolEvent;
use alloy_primitives::B256;

alloy::sol! {
    interface ICamelotPair {
        function getReserves() external view returns (uint112 reserve0, uint112 reserve1, uint16 token0FeePercent, uint16 token1FeePercent);
        function stableSwap() external view returns (bool);
        function precisionMultiplier0() external view returns (uint256);
        function precisionMultiplier1() external view returns (uint256);
        function getAmountOut(uint256 amountIn, address tokenIn) external view returns (uint256);

        event FeePercentUpdated(uint16 token0FeePercent, uint16 token1FeePercent);
        event SetStableSwap(bool prevStableSwap, bool stableSwap);
        event Mint(address indexed sender, uint256 amount0, uint256 amount1);
        event Burn(address indexed sender, uint256 amount0, uint256 amount1, address indexed to);
        event Swap(address indexed sender, uint256 amount0In, uint256 amount1In, uint256 amount0Out, uint256 amount1Out, address indexed to);
        event Sync(uint112 reserve0, uint112 reserve1);
    }
}

/// topic0 событий, меняющих состояние пары: Sync (резервы), FeePercentUpdated (комиссии), SetStableSwap (вид кривой).
/// Swap / Mint / Burn состояния не несут — после них пара сама шлёт Sync (_update, Pair.sol L129-L135). Ошибок нет.
pub fn topics() -> Vec<B256> {
    use ICamelotPair as P;
    vec![
        P::Sync::SIGNATURE_HASH,
        P::FeePercentUpdated::SIGNATURE_HASH,
        P::SetStableSwap::SIGNATURE_HASH,
    ]
}

/// topic0 добавления ликвидности (Mint — как у V2; мелкие пулы слушают только его, depth.rs). Ошибок нет.
pub fn liquidity_topics() -> Vec<B256> {
    vec![ICamelotPair::Mint::SIGNATURE_HASH]
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::sol_types::SolCall;
    use alloy_primitives::keccak256;

    /// Селекторы и topic0 — keccak256 подписей из verified-исходника; Sync / Mint / Swap / Burn — те же, что у V2;
    /// getAmountOut — тот же селектор, что у Solidly (квотер берёт ISolidlyPool).
    #[test]
    fn signatures_match_official() {
        let sel = |s: &str| keccak256(s.as_bytes())[..4].to_vec();
        let ev = |s: &str| B256::from(keccak256(s.as_bytes()));
        use ICamelotPair as P;
        assert_eq!(P::getReservesCall::SELECTOR.to_vec(), sel("getReserves()"));
        assert_eq!(P::stableSwapCall::SELECTOR.to_vec(), sel("stableSwap()"));
        assert_eq!(
            P::getAmountOutCall::SELECTOR,
            super::super::abi_solidly::ISolidlyPool::getAmountOutCall::SELECTOR
        );
        assert_eq!(
            P::FeePercentUpdated::SIGNATURE_HASH,
            ev("FeePercentUpdated(uint16,uint16)")
        );
        assert_eq!(
            P::SetStableSwap::SIGNATURE_HASH,
            ev("SetStableSwap(bool,bool)")
        );
        use super::super::abi_v2v3::IV2Pair;
        assert_eq!(P::Sync::SIGNATURE_HASH, IV2Pair::Sync::SIGNATURE_HASH);
        assert_eq!(P::Mint::SIGNATURE_HASH, IV2Pair::Mint::SIGNATURE_HASH);
        assert_eq!(P::Swap::SIGNATURE_HASH, IV2Pair::Swap::SIGNATURE_HASH);
    }
}
