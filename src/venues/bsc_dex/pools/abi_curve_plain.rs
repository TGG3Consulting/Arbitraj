//! Файл: venues/bsc_dex/pools/abi_curve_plain.rs
//! Что делает: решение 472 — ABI Curve StableSwap 2021 (вариант CurvePlain вида curve_stableswap_plain: 2pool Arbitrum
//! и Plain*Basic фабрики Curve Sidechain/L2) сверх ABI Curve 2020 (abi_curve.rs, геттеры coins/balances/fee, get_dy,
//! exchange — те же): `A_precise()` (A·A_PRECISION — им считает пул), события, которых у Curve 2020 нет или они другие
//! (RemoveLiquidityOne с token_supply, CommitNewFee), и реестр фабрики `get_implementation_address(pool)`.
//! Кто вызывает: pools/load_curve_plain.rs, pools/verify_curve_plain.rs, pools/events_curve.rs (topics_of).
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
//! не этой фабрики).

alloy::sol! {
    interface ICurvePlain {
        event RemoveLiquidityOne(address indexed provider, uint256 token_amount, uint256 coin_amount, uint256 token_supply);
        event CommitNewFee(uint256 indexed deadline, uint256 fee, uint256 admin_fee);

        function A_precise() external view returns (uint256);
    }

    interface ICurvePlainFactory {
        function get_implementation_address(address pool) external view returns (address);
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
    }
}
