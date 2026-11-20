//! Файл: venues/bsc_dex/pools/abi_crypto_v1.rs
//! Что делает: ABI crypto-пулов Curve первого поколения (решение 453) — фабрика crypto v1 (CurveCryptoSwap2ETH.vy,
//! Vyper 0.3.1) и tricrypto 2021 (CryptoSwap.vy, Vyper 0.2.15): геттеры, которых нет у NG (price_scale() двухмонетного
//! v1 — публичная переменная, is_killed() tricrypto), реестр фабрики v1 (get_coins), событие создания пула фабрики
//! CryptoPoolDeployed (адреса пула в нём нет — пул ищется по фабрике), topic0 событий пулов v1 (у v1 свои подписи:
//! TokenExchange из 5 полей, AddLiquidity / RemoveLiquidity без uint256 «price», NewParameters из 7 полей). Общие геттеры
//! (coins, balances, D, A, gamma, mid_fee, out_fee, fee_gamma, future_A_gamma_time, get_dy) — ICryptoPool
//! (abi_crypto.rs), подписи те же.
//! Кто вызывает: pools/load_crypto_v1.rs, pools/verify_crypto_v1.rs, pools/events_crypto.rs, pools/created_crypto.rs.
//! От чего зависит: alloy::sol!, alloy-primitives (keccak256).
//! Настройки: нет.
//!
//! Источники:
//! - curvefi/curve-factory-crypto 7ed1011, contracts/CurveCryptoSwap2ETH.vy: события L25-L83 (TokenExchange L25,
//!   AddLiquidity L32, RemoveLiquidity L38, RemoveLiquidityOne L43, NewParameters L59, RampAgamma L68, StopRampA L76,
//!   ClaimAdminFee L81), price_scale L115, future_A_gamma_time L124, exchange L859-L866; contracts/Factory.vy:
//!   CryptoPoolDeployed L38-L51, find_pool_for_coins L335-L345, get_coins L353-L360.
//! - curvefi/curve-crypto-contract a0f8401, «deployment-logs/2021-09-13. Arbitrum/CryptoSwap.vy»: события L37-L104
//!   (те же поля с uint256[3]), is_killed L160, price_scale(k) L271-L274, exchange L591-L593.

use std::sync::LazyLock;

use alloy_primitives::{B256, keccak256};

// События и функции контрактов Curve несут больше 7 полей — clippy::too_many_arguments на сгенерированных
// конструкторах alloy::sol! (код макроса, не наш).
#[allow(clippy::too_many_arguments)]
mod sol {
    alloy::sol! {
        interface ICryptoPoolV1 {
            function price_scale() external view returns (uint256);
            function is_killed() external view returns (bool);
        }

        // Решение 472 (T472-03): геттеры tweak_price / _claim_admin_fees пула crypto v1 фабрики —
        // curvefi/curve-factory-crypto 7ed1011 CurveCryptoSwap2ETH.vy L112 token, L126 allowed_extra_profit,
        // L132 adjustment_step, L140 admin_fee, L148 factory, L150-L152 xcp_profit / xcp_profit_a / virtual_price,
        // L1341 price_oracle(); Factory.vy L108 fee_receiver; CurveTokenV5.vy L42 totalSupply.
        interface ICryptoTweakV1 {
            function token() external view returns (address);
            function factory() external view returns (address);
            function price_oracle() external view returns (uint256);
            function xcp_profit() external view returns (uint256);
            function xcp_profit_a() external view returns (uint256);
            function virtual_price() external view returns (uint256);
            function allowed_extra_profit() external view returns (uint256);
            function adjustment_step() external view returns (uint256);
            function admin_fee() external view returns (uint256);
            function fee_receiver() external view returns (address);
            function totalSupply() external view returns (uint256);
        }

        // Решение 472 (T472-03): геттеры tweak_price пулов без фабрики — пара CurveCryptoSwap2 (EURS/2CRV, curvefi/
        // curve-crypto-contract 41e6289 «2021-10-30. EURS/CryptoSwap.vy»: L98 price_oracle (сохранённый), L100
        // last_prices, L101 last_prices_timestamp, L117 ma_half_time, L143 admin_fee_receiver) и tricrypto 2021
        // («2021-09-13. Arbitrum/CryptoSwap.vy» L123 last_prices_timestamp, L139 ma_half_time, L267-L280
        // price_oracle(k) / last_prices(k) — упакованные сохранённые).
        interface ICryptoTweakPair {
            function last_prices() external view returns (uint256);
            function last_prices_timestamp() external view returns (uint256);
            function ma_half_time() external view returns (uint256);
            function admin_fee_receiver() external view returns (address);
        }

        interface ICryptoTweakTri {
            function last_prices(uint256 k) external view returns (uint256);
            function price_oracle(uint256 k) external view returns (uint256);
        }

        interface ICryptoFactoryV1 {
            function get_coins(address pool) external view returns (address[2] memory);

            event CryptoPoolDeployed(
                address token,
                address[2] coins,
                uint256 A,
                uint256 gamma,
                uint256 mid_fee,
                uint256 out_fee,
                uint256 allowed_extra_profit,
                uint256 fee_gamma,
                uint256 adjustment_step,
                uint256 admin_fee,
                uint256 ma_half_time,
                uint256 initial_price,
                address deployer
            );
        }
    }
}
pub use sol::*;

/// Подписи событий изменения ликвидности пулов v1 (2 и 3 монеты), которых нет у NG — их слушают мелкие пулы
/// (pools/depth.rs). RemoveLiquidity(address,uint256[N],uint256) у v1 и NG одна и та же (abi_crypto.rs).
const LIQUIDITY: [&str; 3] = [
    "AddLiquidity(address,uint256[2],uint256,uint256)",
    "AddLiquidity(address,uint256[3],uint256,uint256)",
    "RemoveLiquidityOne(address,uint256,uint256,uint256)",
];

/// Подписи остальных событий пулов v1, после которых пул перечитывается. RampAgamma, StopRampA и ClaimAdminFee — те же
/// подписи, что у NG (abi_crypto.rs), NewParameters из 7 полей — та же, что у CurveTricryptoOptimized.
const OTHER: [&str; 1] = ["TokenExchange(address,uint256,uint256,uint256,uint256)"];

/// topic0 событий ликвидности v1 (посчитаны один раз).
static LIQUIDITY_TOPICS: LazyLock<Vec<B256>> =
    LazyLock::new(|| LIQUIDITY.iter().map(|s| keccak256(s.as_bytes())).collect());

/// topic0 всех своих событий v1 (посчитаны один раз).
static ALL_TOPICS: LazyLock<Vec<B256>> = LazyLock::new(|| {
    let mut out: Vec<B256> = OTHER.iter().map(|s| keccak256(s.as_bytes())).collect();
    out.extend(LIQUIDITY_TOPICS.iter().copied());
    out
});

/// topic0 событий ликвидности пулов v1. Ошибок нет.
pub fn liquidity_topics() -> Vec<B256> {
    LIQUIDITY_TOPICS.to_vec()
}

/// topic0 своих событий пулов v1 (без общих с NG). Ошибок нет.
pub fn topics() -> &'static [B256] {
    &ALL_TOPICS
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::sol_types::SolEvent;

    /// Подписи v1 не совпадают с подписями NG (иначе событие считалось бы дважды) и между собой различны; подпись
    /// CryptoPoolDeployed — из исходника Factory.vy L38-L52.
    #[test]
    fn topics_distinct() {
        let mut all = topics().to_vec();
        all.extend(super::super::abi_crypto::topics().iter().copied());
        let n = all.len();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), n);
        let sig = concat!(
            "CryptoPoolDeployed(address,address[2],uint256,uint256,uint256,uint256,uint256,",
            "uint256,uint256,uint256,uint256,uint256,address)"
        );
        let want = keccak256(sig.as_bytes());
        assert_eq!(ICryptoFactoryV1::CryptoPoolDeployed::SIGNATURE_HASH, want);
    }
}
