//! Файл: venues/bsc_dex/pools/abi_curve_plain.rs
//! Что делает: решение 472 — ABI Curve StableSwap 2021 (вариант CurvePlain вида curve_stableswap_plain: 2pool Arbitrum
//! и Plain*Basic фабрики Curve Sidechain/L2) сверх ABI Curve 2020 (abi_curve.rs, геттеры coins/balances/fee, get_dy,
//! exchange — те же): `A_precise()` (A·A_PRECISION — им считает пул), события, которых у Curve 2020 нет или они другие
//! (RemoveLiquidityOne с token_supply, CommitNewFee), и реестр фабрики `get_implementation_address(pool)`.
//! Кто вызывает: pools/load_curve_plain.rs, load_curve_meta.rs, load_curve_oracle.rs, verify_curve_plain.rs,
//! events_curve.rs (topics_of).
//! От чего зависит: alloy::sol!.
//! Настройки: нет.
//!
//! Источники: curvefi/curve-factory f389123 contracts/implementations/plain-2/Plain2Basic.vy (Vyper 0.2.15; Sourcify
//! 42161 match реализации 0x54e8A25d0Ac0E4945b697C80b8372445FEA17A62): события L18-L71 (RemoveLiquidityOne — provider,
//! token_amount, coin_amount, token_supply), `A_precise()` L264-L265 (`return self._A()`), `A()` L258-L259 (`_A() /
//! A_PRECISION` — округлён, для расчёта не годится). 2pool Arbitrum 0x7f90122BF0700F9E7e1F688fe926940E8839F353 (Vyper
//! 0.2.12, Sourcify 42161 match; ABI — curvefi/curve-js ab3f1c2 src/constants/abis/2pool/swap.json): те же события и
//! ещё CommitNewFee(uint256 indexed deadline, uint256 fee, uint256 admin_fee), NewFee(uint256, uint256).
//! Фабрика: curvefi/curve-factory fb61207 contracts/FactorySidechains.vy (Vyper 0.2.16; Sourcify 42161 match
//! 0xb17b674D9c5CB2e441F8e196a2f048A81355d031): `get_implementation_address(_pool) -> address` L451-L458 (ноль — пул
//! не этой фабрики), `get_base_pool(_pool) -> address` L180-L188 (базовый пул метапула).
//!
//! Решение 472 (доделка): реализации с оракулом цены (Plain2BasicEMA / Plain2ETHEMA, curve-factory 5272fd2,
//! файлы последний раз менялись в dfdf855, Vyper 0.3.7; адреса Arbitrum 0x73Ec…AB81 / 0x6F9f…E8C5 — curve-js
//! ab3f1c2 src/constants/factory/stable.ts L169, L172):
//! `ma_exp_time: public(uint256)` L129 (признак реализации с save_p в exchange — во всех реализациях репозитория
//! curve-factory, где есть ma_exp_time, exchange зовёт save_p с одной и той же _get_p), события
//! CommitNewFee(uint256 new_fee) L74-L75 и ApplyNewFee(uint256 fee) L77-L78. Метапул (MetaUSD.vy f389123
//! L48-L53): TokenExchangeUnderlying — те же поля, что TokenExchange.

alloy::sol! {
    interface ICurvePlain {
        event RemoveLiquidityOne(address indexed provider, uint256 token_amount, uint256 coin_amount, uint256 token_supply);
        event CommitNewFee(uint256 indexed deadline, uint256 fee, uint256 admin_fee);

        function A_precise() external view returns (uint256);
    }

    interface ICurvePlainEma {
        event CommitNewFee(uint256 new_fee);
        event ApplyNewFee(uint256 fee);

        function ma_exp_time() external view returns (uint256);
    }

    interface ICurveMeta {
        event TokenExchangeUnderlying(address indexed buyer, int128 sold_id, uint256 tokens_sold, int128 bought_id, uint256 tokens_bought);

        function get_virtual_price() external view returns (uint256);
    }

    interface ICurveOraclePool {
        function oracle() external view returns (address);
    }

    interface IRateOracle {
        function latestAnswer() external view returns (int256);
    }

    interface ICurvePlainFactory {
        function get_implementation_address(address pool) external view returns (address);
        function get_base_pool(address pool) external view returns (address);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::sol_types::{SolCall, SolEvent};
    use alloy_primitives::{B256, keccak256};

    /// Селекторы и topic0 совпадают с keccak256 сигнатур официальных исходников (см. шапку).
    #[test]
    fn signatures_match_official() {
        let topic = |sig: &str| B256::from(keccak256(sig.as_bytes()));
        let sel = |sig: &str| keccak256(sig.as_bytes())[..4].to_vec();
        assert_eq!(
            ICurvePlain::RemoveLiquidityOne::SIGNATURE_HASH,
            topic("RemoveLiquidityOne(address,uint256,uint256,uint256)")
        );
        assert_eq!(
            ICurvePlain::CommitNewFee::SIGNATURE_HASH,
            topic("CommitNewFee(uint256,uint256,uint256)")
        );
        assert_eq!(
            ICurvePlain::A_preciseCall::SELECTOR.to_vec(),
            sel("A_precise()")
        );
        assert_eq!(
            ICurvePlainFactory::get_implementation_addressCall::SELECTOR.to_vec(),
            sel("get_implementation_address(address)")
        );
        assert_eq!(
            ICurvePlainFactory::get_base_poolCall::SELECTOR.to_vec(),
            sel("get_base_pool(address)")
        );
        assert_eq!(
            ICurvePlainEma::CommitNewFee::SIGNATURE_HASH,
            topic("CommitNewFee(uint256)")
        );
        assert_eq!(
            ICurvePlainEma::ApplyNewFee::SIGNATURE_HASH,
            topic("ApplyNewFee(uint256)")
        );
        assert_eq!(
            ICurvePlainEma::ma_exp_timeCall::SELECTOR.to_vec(),
            sel("ma_exp_time()")
        );
        assert_eq!(
            ICurveMeta::TokenExchangeUnderlying::SIGNATURE_HASH,
            topic("TokenExchangeUnderlying(address,int128,uint256,int128,uint256)")
        );
        assert_eq!(
            ICurveMeta::get_virtual_priceCall::SELECTOR.to_vec(),
            sel("get_virtual_price()")
        );
        assert_eq!(
            ICurveOraclePool::oracleCall::SELECTOR.to_vec(),
            sel("oracle()")
        );
        assert_eq!(
            IRateOracle::latestAnswerCall::SELECTOR.to_vec(),
            sel("latestAnswer()")
        );
    }
}
