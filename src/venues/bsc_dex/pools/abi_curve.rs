//! Файл: venues/bsc_dex/pools/abi_curve.rs
//! Что делает: ABI пула StableSwap шаблона Curve 2020 — Ellipsis StableSwap (решение 122 F): геттеры для чтения
//! состояния (load_curve.rs), проверки пула (verify_curve.rs), квотер — сам пул get_dy (quote/quoter_calls.rs), и
//! topic0 событий пула (events_curve.rs). У событий ликвидности в сигнатуре массивы `uint256[N_COINS]` — topic0
//! зависит от числа монет пула, поэтому они собираются из текста сигнатуры для каждого N (topics_for).
//! Кто вызывает: pools/load_curve.rs, pools/verify_curve.rs, pools/events_curve.rs, quote/quoter_calls.rs.
//! От чего зависит: alloy::sol!, alloy-primitives (keccak256).
//! Настройки: нет (адреса пулов — из справочника и dex.stable_pools).
//!
//! Источник (решение 23): ellipsis-finance/ellipsis HEAD 6bdf778, contracts/StableSwap.vy (@version 0.2.11):
//! события L18-L76 (TokenExchange, AddLiquidity, RemoveLiquidity, RemoveLiquidityOne, RemoveLiquidityImbalance,
//! NewFee, RampA, StopRampA); `coins: public(address[N_COINS])` и `balances: public(uint256[N_COINS])` L95-L96 —
//! геттеры Vyper 0.2 с аргументом uint256; `fee: public(uint256)` L97 (над 10^10); `A()` L176-L177 (сырой A);
//! `get_dy(int128, int128, uint256)` L405-L416; `exchange(int128, int128, uint256, uint256)` L434-L436.
//! MAX_COINS = 8 — предел числа монет пула Curve (curvefi/curve-pool-registry, contracts/Registry.vy,
//! `MAX_COINS: constant(int128) = 8`): дальше этого индекса coins(k) не читается.

use alloy_primitives::{B256, keccak256};

alloy::sol! {
    interface IStableSwapV1 {
        event TokenExchange(address indexed buyer, int128 sold_id, uint256 tokens_sold, int128 bought_id, uint256 tokens_bought);
        event RemoveLiquidityOne(address indexed provider, uint256 token_amount, uint256 coin_amount);
        event NewFee(uint256 fee, uint256 admin_fee);
        event RampA(uint256 old_A, uint256 new_A, uint256 initial_time, uint256 future_time);
        event StopRampA(uint256 A, uint256 t);

        function coins(uint256 i) external view returns (address);
        function balances(uint256 i) external view returns (uint256);
        function A() external view returns (uint256);
        function fee() external view returns (uint256);
        function get_dy(int128 i, int128 j, uint256 dx) external view returns (uint256);
        function exchange(int128 i, int128 j, uint256 dx, uint256 min_dy) external;
    }
}

/// Предел числа монет пула Curve (curve-pool-registry Registry.vy MAX_COINS = 8). Ошибок нет.
pub const MAX_COINS: usize = 8;

/// topic0 событий ликвидности с массивами `uint256[n]` для пула из n монет: AddLiquidity, RemoveLiquidity,
/// RemoveLiquidityImbalance (StableSwap.vy L25-L48). Возвращает: (add, остальные). Ошибок нет.
pub fn topics_for(n: usize) -> (B256, Vec<B256>) {
    let h = |s: String| B256::from(keccak256(s.as_bytes()));
    let add = h(format!(
        "AddLiquidity(address,uint256[{n}],uint256[{n}],uint256,uint256)"
    ));
    let rest = vec![
        h(format!(
            "RemoveLiquidity(address,uint256[{n}],uint256[{n}],uint256)"
        )),
        h(format!(
            "RemoveLiquidityImbalance(address,uint256[{n}],uint256[{n}],uint256,uint256)"
        )),
    ];
    (add, rest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::sol_types::{SolCall, SolEvent};

    /// Селекторы и topic0 совпадают с keccak256 сигнатур официального StableSwap.vy.
    #[test]
    fn signatures_match_official() {
        let topic = |sig: &str| B256::from(keccak256(sig.as_bytes()));
        let sel = |sig: &str| keccak256(sig.as_bytes())[..4].to_vec();
        assert_eq!(
            IStableSwapV1::TokenExchange::SIGNATURE_HASH,
            topic("TokenExchange(address,int128,uint256,int128,uint256)")
        );
        assert_eq!(
            IStableSwapV1::RampA::SIGNATURE_HASH,
            topic("RampA(uint256,uint256,uint256,uint256)")
        );
        assert_eq!(
            IStableSwapV1::coinsCall::SELECTOR.to_vec(),
            sel("coins(uint256)")
        );
        assert_eq!(
            IStableSwapV1::get_dyCall::SELECTOR.to_vec(),
            sel("get_dy(int128,int128,uint256)")
        );
        assert_eq!(
            IStableSwapV1::exchangeCall::SELECTOR.to_vec(),
            sel("exchange(int128,int128,uint256,uint256)")
        );
        let (add, _) = topics_for(3);
        assert_eq!(
            add,
            topic("AddLiquidity(address,uint256[3],uint256[3],uint256,uint256)")
        );
    }
}
