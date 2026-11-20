//! Файл: venues/bsc_dex/pools/abi_saddle.rs
//! Что делает: ABI пула Saddle StableSwap — Nerve (решение 122 F, 3): геттеры монет и балансов по индексу uint8, A в
//! точности A_PRECISION, swapStorage (комиссия), пауза, квотер calculateSwap, обмен swap с deadline и topic0 событий
//! (обмен, ликвидность с динамическими массивами uint256[], комиссии, рампа A, пауза — любое → перечитать пул).
//! Кто вызывает: pools/load_saddle.rs, pools/verify_saddle.rs, pools/events_stable_u.rs, quote/quoter_calls.rs.
//! От чего зависит: alloy::sol!.
//! Настройки: нет (адреса пулов — dex.stable_pools).
//!
//! Источник (официальный, решение 23): nerve-finance/contracts bda7578 — Swap.sol: `swapStorage` L37 (публичный
//! геттер структуры SwapUtils.Swap без массивов и отображений: initialA, futureA, initialATime, futureATime, swapFee,
//! adminFee, defaultDepositFee, defaultWithdrawFee, devaddr, lpToken), события L47-L90, `getAPrecise` L222,
//! `getToken(uint8)` L231 (откат «Out of range» за последним индексом), `getTokenBalance(uint8)` L264,
//! `calculateSwap(uint8,uint8,uint256)` L285, `swap(uint8,uint8,uint256,uint256,uint256 deadline)` L389 (whenNotPaused);
//! пулов не больше 32 монет — L125; SwapUtils.sol `struct Swap` L69-L95; OwnerPausable.sol — OpenZeppelin Pausable.
//!
//! Решение 451 (T448-07, Saddle на Arbitrum): saddle-finance/saddle-contract 2cfdd055 — пулы deployments/arbitrum_mainnet
//! (SaddleArbUSDPool, SaddleArbUSDPoolV2, SaddleFRAXBPPool — SwapFlashLoan; CommunityfUSDCPool — PermissionlessSwap)
//! связаны с библиотеками SwapUtils 0xcF70…6894 и AmplificationUtils 0x4f6A…c62e (solcInputs/83663b3e…; тексты
//! библиотек = contracts/SwapUtils.sol, AmplificationUtils.sol, MathUtils.sol коммита). Математика совпадает с Nerve
//! (SwapUtils.sol getD L307-L357, getY L428-L480, _calculateSwap L519-L540, MAX_LOOP_LIMIT L124, A_PRECISION —
//! AmplificationUtils.sol L25, _getAPrecise L60-L88). Отличия ABI: (1) `swapStorage` Saddle — 7 полей (struct Swap
//! SwapUtils.sol L61-L82 без defaultDepositFee/defaultWithdrawFee/devaddr; Swap.sol L38; ABI deployments/
//! arbitrum_mainnet/*.json) — swapFee в обоих пятым словом, разбор по длине ответа (swap_fee); (2) у SwapFlashLoan
//! событие FlashLoan (SwapFlashLoan.sol L36-L42) — flashLoan меняет balances[tokenIndex] (L98-L141): пул перечитывается.
//! Остальные геттеры, swap, calculateSwap, события обмена и ликвидности, RampA/StopRampA, Paused/Unpaused — те же.

use alloy_primitives::U256;

alloy::sol! {
    /// Пул Saddle (Nerve).
    interface ISaddleSwap {
        event TokenSwap(address indexed buyer, uint256 tokensSold, uint256 tokensBought, uint128 soldId, uint128 boughtId);
        event AddLiquidity(address indexed provider, uint256[] tokenAmounts, uint256[] fees, uint256 invariant, uint256 lpTokenSupply);
        event RemoveLiquidity(address indexed provider, uint256[] tokenAmounts, uint256 lpTokenSupply);
        event RemoveLiquidityOne(address indexed provider, uint256 lpTokenAmount, uint256 lpTokenSupply, uint256 boughtId, uint256 tokensBought);
        event RemoveLiquidityImbalance(address indexed provider, uint256[] tokenAmounts, uint256[] fees, uint256 invariant, uint256 lpTokenSupply);
        event NewAdminFee(uint256 newAdminFee);
        event NewSwapFee(uint256 newSwapFee);
        event RampA(uint256 oldA, uint256 newA, uint256 initialTime, uint256 futureTime);
        event StopRampA(uint256 currentA, uint256 time);
        event Paused(address account);
        event Unpaused(address account);

        function swapStorage() external view returns (uint256 initialA, uint256 futureA, uint256 initialATime, uint256 futureATime, uint256 swapFee, uint256 adminFee, uint256 defaultDepositFee, uint256 defaultWithdrawFee, address devaddr, address lpToken);
        function getAPrecise() external view returns (uint256);
        function getToken(uint8 index) external view returns (address);
        function getTokenBalance(uint8 index) external view returns (uint256);
        function paused() external view returns (bool);
        function calculateSwap(uint8 tokenIndexFrom, uint8 tokenIndexTo, uint256 dx) external view returns (uint256);
        function swap(uint8 tokenIndexFrom, uint8 tokenIndexTo, uint256 dx, uint256 minDy, uint256 deadline) external returns (uint256);
    }

    /// Пул Saddle (saddle-contract 2cfdd055, решение 451): swapStorage из 7 полей и FlashLoan у SwapFlashLoan.
    interface ISaddleOwn {
        event FlashLoan(address indexed receiver, uint8 tokenIndex, uint256 amount, uint256 amountFee, uint256 protocolFee);
        function swapStorage() external view returns (uint256 initialA, uint256 futureA, uint256 initialATime, uint256 futureATime, uint256 swapFee, uint256 adminFee, address lpToken);
    }
}

/// Решение 451: swapFee из ответа swapStorage() — Nerve (10 слов, ISaddleSwap) или Saddle (7 слов, ISaddleOwn); обе
/// раскладки — статический кортеж, swapFee пятым словом. Принимает: байты ответа. Возвращает: swapFee; None — длина
/// не 7 и не 10 слов или ответ не по ABI.
pub fn swap_fee(out: &[u8]) -> Option<U256> {
    use alloy::sol_types::SolCall;
    match out.len() {
        320 => ISaddleSwap::swapStorageCall::abi_decode_returns(out)
            .ok()
            .map(|r| r.swapFee),
        224 => ISaddleOwn::swapStorageCall::abi_decode_returns(out)
            .ok()
            .map(|r| r.swapFee),
        _ => None,
    }
}

/// Предел числа монет пула Saddle (Swap.sol L125). Ошибок нет.
pub const MAX_TOKENS: usize = 32;

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::sol_types::{SolCall, SolEvent};
    use alloy_primitives::{B256, keccak256};

    /// Селекторы и topic0 совпадают с keccak256 сигнатур Swap.sol.
    #[test]
    fn signatures_match_official() {
        let sel = |sig: &str| keccak256(sig.as_bytes())[..4].to_vec();
        let topic = |sig: &str| B256::from(keccak256(sig.as_bytes()));
        assert_eq!(
            ISaddleSwap::swapCall::SELECTOR.to_vec(),
            sel("swap(uint8,uint8,uint256,uint256,uint256)")
        );
        assert_eq!(
            ISaddleSwap::calculateSwapCall::SELECTOR.to_vec(),
            sel("calculateSwap(uint8,uint8,uint256)")
        );
        assert_eq!(
            ISaddleSwap::TokenSwap::SIGNATURE_HASH,
            topic("TokenSwap(address,uint256,uint256,uint128,uint128)")
        );
        assert!(format!("{:x}", ISaddleSwap::TokenSwap::SIGNATURE_HASH).starts_with("c6c1e063"));
        // Решение 451: Saddle (saddle-contract SwapFlashLoan.sol L36-L42, Swap.sol L38).
        assert_eq!(
            ISaddleOwn::FlashLoan::SIGNATURE_HASH,
            topic("FlashLoan(address,uint8,uint256,uint256,uint256)")
        );
        assert_eq!(ISaddleOwn::swapStorageCall::SELECTOR, ISaddleSwap::swapStorageCall::SELECTOR);
    }
}
