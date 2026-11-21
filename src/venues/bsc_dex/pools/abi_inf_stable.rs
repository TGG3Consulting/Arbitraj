//! Файл: venues/bsc_dex/pools/abi_inf_stable.rs
//! Что делает: ABI крючка PancakeSwap Infinity Stable (CLStableSwapHook — пул StableSwap-NG, который живёт в крючке
//! пула Infinity CL; решение «122 (Infinity Stable)») — геттеры состояния, квотер get_dy и события для подписки.
//! Кто вызывает: pools/load_inf_stable.rs, pools/verify_inf_stable.rs, pools/events_stable_u.rs, quote/quoter_calls.rs
//! (get_dy — тот же селектор, что IStableSwapV1::get_dy(int128,int128,uint256), abi_curve.rs).
//! От чего зависит: alloy::sol!.
//! Настройки: нет.
//!
//! Источник (официальный: verified-код BscScan, решение 23): CLStableSwapHook 0x86c3DC08FB5a6663cCa15551575d5429e5Efc017
//! https://bscscan.com/address/0x86c3DC08FB5a6663cCa15551575d5429e5Efc017#code (Vyper 0.3.10, sha256
//! 28031f9aee0da15163a6e1fbc2841d6fcf3a55900e3c322975359d6f6024f17b). Геттеры: N_COINS L138 (public immutable),
//! coins L143 (public immutable DynArray → coins(uint256)), fee L149, offpeg_fee_multiplier L150, initial_A/future_A/
//! initial_A_time/future_A_time L160-L163, A_precise L1267-L1270, balances(uint256) L1273-L1282 (stored − admin),
//! stored_rates L1291-L1294 (внешние вызовы оракулов курса и convertToAssets), get_dy L1188-L1199, pool_manager L1475,
//! vault L1476, getHooksRegistrationBitmap L1585-L1605. События L78-L130: DynArray в событии — uint256[] в ABI.
//! Нет геттеров: asset_types, rate_oracles, rate_multipliers, factory (immutable без public) — вид монеты не читается.

// Код функций и событий генерирует alloy::sol! — число аргументов задаёт ABI контракта, а не мы.
#![allow(clippy::too_many_arguments)]

alloy::sol! {
    interface IInfStableHook {
        function N_COINS() external view returns (uint256);
        function coins(uint256 i) external view returns (address);
        function balances(uint256 i) external view returns (uint256);
        function stored_rates() external view returns (uint256[] memory);
        function A_precise() external view returns (uint256);
        function fee() external view returns (uint256);
        function offpeg_fee_multiplier() external view returns (uint256);
        function pool_manager() external view returns (address);
        function getHooksRegistrationBitmap() external pure returns (uint16);
        function get_dy(int128 i, int128 j, uint256 dx) external view returns (uint256);

        event TokenExchange(address indexed buyer, int128 sold_id, uint256 tokens_sold, int128 bought_id, uint256 tokens_bought, uint256 fee);
        event AddLiquidity(address indexed provider, uint256[] token_amounts, uint256[] fees, uint256 invariant, uint256 token_supply);
        event RemoveLiquidity(address indexed provider, uint256[] token_amounts, uint256[] fees, uint256 token_supply);
        event RemoveLiquidityOne(address indexed provider, int128 token_id, uint256 token_amount, uint256 coin_amount, uint256 fee, uint256 token_supply);
        event RemoveLiquidityImbalance(address indexed provider, uint256[] token_amounts, uint256[] fees, uint256 invariant, uint256 token_supply);
        event RampA(uint256 old_A, uint256 new_A, uint256 initial_time, uint256 future_time);
        event StopRampA(uint256 A, uint256 t);
        event ApplyNewFee(uint256 fee, uint256 offpeg_fee_multiplier);
    }
}

/// Монет в пуле крючка: MAX_COINS = 2 (L133); конструктор требует coins[0] < coins[1] (L1530). Ошибок нет.
pub const HOOK_COINS: usize = 2;

/// Биты разрешений крючка в parameters PoolKey, которые нужны обмену: beforeSwap (6) и beforeSwapReturnsDelta (10)
/// (HOOKS_*_OFFSET L1429, L1433; getHooksRegistrationBitmap L1585-L1605 — 0x0455 вместе с beforeInitialize,
/// beforeAddLiquidity, beforeRemoveLiquidity). Ошибок нет.
pub const SWAP_BITS: u16 = (1 << 6) | (1 << 10);

/// topic0 событий изменения ликвидности (их слушают мелкие пулы, pools/depth.rs). Ошибок нет.
pub fn liquidity_topics() -> Vec<alloy_primitives::B256> {
    use IInfStableHook as H;
    use alloy::sol_types::SolEvent;
    vec![
        H::AddLiquidity::SIGNATURE_HASH,
        H::RemoveLiquidity::SIGNATURE_HASH,
        H::RemoveLiquidityOne::SIGNATURE_HASH,
        H::RemoveLiquidityImbalance::SIGNATURE_HASH,
    ]
}

/// topic0 всех событий крючка, после которых пул перечитывается (обмен, ликвидность, A, комиссия). Ошибок нет.
pub fn topics() -> Vec<alloy_primitives::B256> {
    use IInfStableHook as H;
    use alloy::sol_types::SolEvent;
    let mut out = vec![
        H::TokenExchange::SIGNATURE_HASH,
        H::RampA::SIGNATURE_HASH,
        H::StopRampA::SIGNATURE_HASH,
        H::ApplyNewFee::SIGNATURE_HASH,
    ];
    out.extend(liquidity_topics());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::sol_types::SolCall;

    /// get_dy крючка — тот же селектор, что у квотера StableSwap Curve (abi_curve.rs): квотер общий.
    #[test]
    fn get_dy_selector_is_curve() {
        assert_eq!(
            IInfStableHook::get_dyCall::SELECTOR,
            super::super::abi_curve::IStableSwapV1::get_dyCall::SELECTOR
        );
    }
}
