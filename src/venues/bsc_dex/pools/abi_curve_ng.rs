//! Файл: venues/bsc_dex/pools/abi_curve_ng.rs
//! Что делает: ABI пула Curve StableSwap-NG (plain pool, version() = "v7.0.0") и его фабрики (решение 275, curve-bsc):
//! геттеры состояния (load_curve_ng.rs), квотер — сам пул get_dy(int128, int128, uint256) (тот же селектор, что у
//! Curve 2020, abi_curve.rs), реестр фабрики (verify_curve_ng.rs) и topic0 событий пула (events_curve_ng.rs).
//! Кто вызывает: pools/load_curve_ng.rs, pools/verify_curve_ng.rs, pools/events_curve_ng.rs.
//! От чего зависит: alloy::sol!.
//! Настройки: нет (фабрика — dex.factory [DEX]).
//!
//! Источник (решение 23): официальный репозиторий curvefi/stableswap-ng, коммит 3332cd44656ec64b3c048885e1dd71955254b262,
//! contracts/main/CurveStableSwapNG.vy (version "v7.0.0" L216): события L92-L145 (TokenExchange — 5 полей, как у
//! Curve 2020; AddLiquidity / RemoveLiquidity / RemoveLiquidityImbalance — DynArray → uint256[] в ABI;
//! RemoveLiquidityOne — 5 полей; RampA, StopRampA, ApplyNewFee), N_COINS L157 (public immutable), coins L162 (public
//! immutable DynArray → coins(uint256)), version L216, fee L169, offpeg_fee_multiplier L170, get_dy L1702, A_precise L1781,
//! balances L1787 (`_balances()` — без доли админа; у пула с rebasing-монетой — balanceOf − admin), stored_rates L1805
//! (`_stored_rates()` — курсы монет, у оракульных и ERC4626 — внешний вызов на этом блоке);
//! contracts/main/CurveStableSwapFactoryNG.vy: get_coins L175, is_meta L430. На BSC: фабрика
//! 0xd7E72f3615aa65b92A4DBdC211E296a35512988B (pool_count = 298 на 14.11.2026), пулы отвечают version() = "v7.0.0"
//! (eth_call 14.11.2026, tests/data/curve_ng_2026-11-14).

// Код функций и событий генерирует alloy::sol! — число аргументов задаёт ABI контракта, а не мы.
#![allow(clippy::too_many_arguments)]

use alloy::sol_types::SolEvent;
use alloy_primitives::B256;

alloy::sol! {
    interface ICurveNgPool {
        function N_COINS() external view returns (uint256);
        function coins(uint256 i) external view returns (address);
        function balances(uint256 i) external view returns (uint256);
        function stored_rates() external view returns (uint256[] memory);
        function A_precise() external view returns (uint256);
        function fee() external view returns (uint256);
        function offpeg_fee_multiplier() external view returns (uint256);
        function get_dy(int128 i, int128 j, uint256 dx) external view returns (uint256);
        function version() external view returns (string memory);

        event TokenExchange(address indexed buyer, int128 sold_id, uint256 tokens_sold, int128 bought_id, uint256 tokens_bought);
        event AddLiquidity(address indexed provider, uint256[] token_amounts, uint256[] fees, uint256 invariant, uint256 token_supply);
        event RemoveLiquidity(address indexed provider, uint256[] token_amounts, uint256[] fees, uint256 token_supply);
        event RemoveLiquidityOne(address indexed provider, int128 token_id, uint256 token_amount, uint256 coin_amount, uint256 token_supply);
        event RemoveLiquidityImbalance(address indexed provider, uint256[] token_amounts, uint256[] fees, uint256 invariant, uint256 token_supply);
        event RampA(uint256 old_A, uint256 new_A, uint256 initial_time, uint256 future_time);
        event StopRampA(uint256 A, uint256 t);
        event ApplyNewFee(uint256 fee, uint256 offpeg_fee_multiplier);
    }

    interface ICurveNgFactory {
        function get_coins(address pool) external view returns (address[] memory);
        function is_meta(address pool) external view returns (bool);
    }
}

/// Предел числа монет пула NG (MAX_COINS = 8, CurveStableSwapNG.vy L152). Ошибок нет.
pub const NG_MAX_COINS: usize = 8;

/// topic0 событий изменения ликвидности (их слушают мелкие пулы, pools/depth.rs). Ошибок нет.
pub fn liquidity_topics() -> Vec<B256> {
    use ICurveNgPool as P;
    vec![
        P::AddLiquidity::SIGNATURE_HASH,
        P::RemoveLiquidity::SIGNATURE_HASH,
        P::RemoveLiquidityOne::SIGNATURE_HASH,
        P::RemoveLiquidityImbalance::SIGNATURE_HASH,
    ]
}

/// topic0 всех событий пула, после которых он перечитывается (обмен, ликвидность, A, комиссия). Ошибок нет.
pub fn topics() -> Vec<B256> {
    use ICurveNgPool as P;
    let mut out = vec![
        P::TokenExchange::SIGNATURE_HASH,
        P::RampA::SIGNATURE_HASH,
        P::StopRampA::SIGNATURE_HASH,
        P::ApplyNewFee::SIGNATURE_HASH,
    ];
    out.extend(liquidity_topics());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::sol_types::SolCall;
    use alloy_primitives::keccak256;

    /// Сигнатуры совпадают с keccak256 текста из CurveStableSwapNG.vy; get_dy и TokenExchange — те же, что у Curve 2020.
    #[test]
    fn signatures_match_official() {
        let topic = |s: &str| B256::from(keccak256(s.as_bytes()));
        assert_eq!(
            ICurveNgPool::AddLiquidity::SIGNATURE_HASH,
            topic("AddLiquidity(address,uint256[],uint256[],uint256,uint256)")
        );
        assert_eq!(
            ICurveNgPool::RemoveLiquidityOne::SIGNATURE_HASH,
            topic("RemoveLiquidityOne(address,int128,uint256,uint256,uint256)")
        );
        assert_eq!(
            ICurveNgPool::TokenExchange::SIGNATURE_HASH,
            super::super::abi_curve::IStableSwapV1::TokenExchange::SIGNATURE_HASH
        );
        assert_eq!(
            ICurveNgPool::get_dyCall::SELECTOR,
            super::super::abi_curve::IStableSwapV1::get_dyCall::SELECTOR
        );
    }
}
