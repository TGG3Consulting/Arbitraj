//! Файл: venues/bsc_dex/pools/abi_crypto.rs
//! Что делает: ABI crypto-пулов Curve (решение 442) — Twocrypto-NG (2 монеты) и Tricrypto-NG (3 монеты, реализации
//! «native transfers enabled» — CurveTricryptoOptimizedWETH.vy и «disabled» — CurveTricryptoOptimized.vy): геттеры
//! состояния (load_crypto.rs), квотер — сам пул get_dy(uint256, uint256, uint256), реестр фабрики (verify_crypto.rs:
//! get_coins; factory_ask.rs: find_pool_for_coins) и topic0 событий пула (events_crypto.rs).
//! Кто вызывает: pools/load_crypto.rs, pools/verify_crypto.rs, pools/events_crypto.rs, pools/factory_ask.rs,
//! quote/quoter_calls.rs.
//! От чего зависит: alloy::sol!, alloy-primitives (keccak256).
//! Настройки: нет (фабрика — dex.factory [DEX]).
//!
//! Источники (решение 23 — официальные репозитории curvefi):
//! - twocrypto-ng 89c8dfb (тег prod-v2.1.0), contracts/main/CurveTwocryptoOptimized.vy: события L57-L119 (TokenExchange
//!   L67, AddLiquidity L76, RemoveLiquidity L83, RemoveLiquidityOne L88, NewParameters L96, RampAgamma L104, StopRampA
//!   L112, ClaimAdminFee L117 — массивы uint256[2]), MATH L128, coins L129, future_A_gamma_time L142, balances L149, D
//!   L150, version L189, exchange L364, get_dy L1522, price_scale() L1596, A L1654, gamma L1664, mid_fee L1674, out_fee
//!   L1684, fee_gamma L1694, precisions() L1736; CurveTwocryptoFactory.vy: find_pool_for_coins L376, get_coins L404.
//!   Решение 472 (модель tweak_price): xcp_profit L151, virtual_price L154, packed_rebalancing_params L158, totalSupply
//!   L193, price_oracle() L1581 (у d612fde — L155, L158, L162, L197, L1653); Tricrypto-NG (CurveTricryptoOptimizedWETH.vy
//!   ecaa816) — xcp_profit L171, virtual_price L174, packed_rebalancing_params L179, totalSupply L219, price_oracle(k)
//!   L1734.
//! - tricrypto-ng ecaa816, contracts/main/CurveTricryptoOptimizedWETH.vy: события L65-L136 (TokenExchange L75,
//!   AddLiquidity L84 и RemoveLiquidity L91 — uint256[3], RemoveLiquidityOne L96, NewParameters L113 — 6 полей, RampAgamma
//!   L121, StopRampA L129, ClaimAdminFee L134 — tokens uint256), MATH L148, coins L149, future_A_gamma_time L162, balances
//!   L169, D L170, version L215, exchange L410, get_dy L1669, price_scale(k) L1788, A L1847, gamma L1857, mid_fee L1867,
//!   out_fee L1877, fee_gamma L1887, precisions() L1929; CurveTricryptoOptimized.vy — те же геттеры (L132-L1889), события
//!   NewParameters L99 (7 полей, + xcp_ma_time) и ClaimAdminFee L121 (tokens uint256[3]); CurveL2TricryptoFactory.vy:
//!   find_pool_for_coins L369, get_coins L387.

// Код функций и событий генерирует alloy::sol! — число аргументов задаёт ABI контракта, а не мы.
#![allow(clippy::too_many_arguments)]

use std::sync::LazyLock;

use alloy_primitives::{B256, keccak256};

alloy::sol! {
    interface ICryptoPool {
        function coins(uint256 i) external view returns (address);
        function balances(uint256 i) external view returns (uint256);
        function D() external view returns (uint256);
        function A() external view returns (uint256);
        function gamma() external view returns (uint256);
        function mid_fee() external view returns (uint256);
        function out_fee() external view returns (uint256);
        function fee_gamma() external view returns (uint256);
        function future_A_gamma_time() external view returns (uint256);
        function version() external view returns (string memory);
        function MATH() external view returns (address);
        function get_dy(uint256 i, uint256 j, uint256 dx) external view returns (uint256);
    }

    interface ITwocryptoPool {
        function price_scale() external view returns (uint256);
        function precisions() external view returns (uint256[2] memory);
        function price_oracle() external view returns (uint256);
        function totalSupply() external view returns (uint256);
        function xcp_profit() external view returns (uint256);
        function virtual_price() external view returns (uint256);
        function packed_rebalancing_params() external view returns (uint256);
    }

    interface ITricryptoPool {
        function price_scale(uint256 k) external view returns (uint256);
        function precisions() external view returns (uint256[3] memory);
        function price_oracle(uint256 k) external view returns (uint256);
        function totalSupply() external view returns (uint256);
        function xcp_profit() external view returns (uint256);
        function virtual_price() external view returns (uint256);
        function packed_rebalancing_params() external view returns (uint256);
    }

    interface ITwocryptoFactory {
        function get_coins(address pool) external view returns (address[2] memory);
    }

    interface ITricryptoFactory {
        function get_coins(address pool) external view returns (address[3] memory);
    }

    interface ICryptoFactory {
        function find_pool_for_coins(address from, address to, uint256 i) external view returns (address);
    }
}

/// Подписи событий изменения ликвидности (их слушают мелкие пулы, pools/depth.rs) — тексты из исходников (шапка).
const LIQUIDITY: [&str; 6] = [
    "AddLiquidity(address,uint256[2],uint256,uint256,uint256)",
    "RemoveLiquidity(address,uint256[2],uint256)",
    "AddLiquidity(address,uint256[3],uint256,uint256,uint256)",
    "RemoveLiquidity(address,uint256[3],uint256)",
    "RemoveLiquidityOne(address,uint256,uint256,uint256,uint256,uint256)",
    "ClaimAdminFee(address,uint256[2])",
];

/// Подписи остальных событий пула, после которых он перечитывается (обмен, параметры, рампа, доля админа).
const OTHER: [&str; 6] = [
    "TokenExchange(address,uint256,uint256,uint256,uint256,uint256,uint256)",
    "NewParameters(uint256,uint256,uint256,uint256,uint256,uint256)",
    "NewParameters(uint256,uint256,uint256,uint256,uint256,uint256,uint256)",
    "RampAgamma(uint256,uint256,uint256,uint256,uint256,uint256)",
    "StopRampA(uint256,uint256,uint256)",
    "ClaimAdminFee(address,uint256)",
];

/// Ещё одна подпись ClaimAdminFee (CurveTricryptoOptimized.vy L121: tokens uint256[3]).
const CLAIM3: &str = "ClaimAdminFee(address,uint256[3])";

/// topic0 событий ликвидности (посчитаны один раз).
static LIQUIDITY_TOPICS: LazyLock<Vec<B256>> = LazyLock::new(|| {
    let mut out: Vec<B256> = LIQUIDITY.iter().map(|s| keccak256(s.as_bytes())).collect();
    out.push(keccak256(CLAIM3.as_bytes()));
    out
});

/// topic0 всех событий пула (посчитаны один раз).
static ALL_TOPICS: LazyLock<Vec<B256>> = LazyLock::new(|| {
    let mut out: Vec<B256> = OTHER.iter().map(|s| keccak256(s.as_bytes())).collect();
    out.extend(LIQUIDITY_TOPICS.iter().copied());
    out
});

/// topic0 событий изменения ликвидности (обоих видов и обеих реализаций Tricrypto). Ошибок нет.
pub fn liquidity_topics() -> Vec<B256> {
    LIQUIDITY_TOPICS.to_vec()
}

/// topic0 всех событий пула, после которых он перечитывается. Ошибок нет.
pub fn topics() -> &'static [B256] {
    &ALL_TOPICS
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::sol_types::SolCall;

    /// Селектор get_dy(uint256,uint256,uint256) — тот же, что у шага STABLE_U256 исполнителя (exchange с индексами
    /// uint256 у пула есть: exchange(uint256,uint256,uint256,uint256)); подписи событий различны.
    #[test]
    fn selectors_and_topics() {
        let h = keccak256(b"get_dy(uint256,uint256,uint256)");
        let sel = &h[..4];
        assert_eq!(ICryptoPool::get_dyCall::SELECTOR.as_slice(), sel);
        let all = topics();
        let mut uniq = all.to_vec();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), all.len());
        assert_eq!(all.len(), 13);
    }
}
