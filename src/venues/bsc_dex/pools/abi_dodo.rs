//! Файл: venues/bsc_dex/pools/abi_dodo.rs
//! Что делает: ABI пула и фабрики DODO PMM (DVM/DPP/DSP, решения 80, 86b) — для eth_call чтения состояния
//! (load_dodo.rs), проверки пула (verify_dodo.rs), разбора событий (events_dodo.rs) и сверки с квотером
//! (quote/quoter_calls.rs — сам пул querySellBase/querySellQuote).
//! Кто вызывает: pools/load_dodo.rs, pools/events_dodo.rs, pools/verify_dodo.rs, pools/created_log.rs,
//! quote/quoter_calls.rs.
//! От чего зависит: alloy::sol!.
//! Настройки: нет (адрес фабрики — dex.factory по виду пула; адрес пула — из справочника).
//!
//! Сигнатуры — из официальных исходников DODOEX/contractV2 (HEAD 2f1bcda, решение 23):
//! - пул: getPMMStateForCall → (i,K,B,Q,B0,Q0,R) (adjustedTarget уже применён), getUserFeeRate(user) →
//!   (lpFeeRate, mtFeeRate), _BASE_TOKEN_/_QUOTE_TOKEN_, getMidPrice — contracts/SmartRoute/intf/IDODOV2.sol и
//!   DVMStorage.sol/DVMVault.sol (getPMMStateForCall L62-L88, getUserFeeRate DVMVault.sol L38-L41);
//! - квотер (сам пул): querySellBase(trader, payBase) / querySellQuote(trader, payQuote) — DVM возвращает
//!   (receive, mtFee), DPP/DSP — (receive, mtFee, RState, newTarget); селектор одинаков, первое слово = выход
//!   (DVMTrader.sol L128-L160, DPPTrader.sol L201-L245, DSPTrader.sol L194-L235);
//! - события: DODOSwap, BuyShares, SellShares, RChange, DODOFlashLoan — DVMTrader.sol / DVMFunding.sol /
//!   DSPTrader.sol (RChange несёт только новое R — состояние всё равно перечитываем, events_dodo.rs);
//! - фабрика: getDODOPoolBidirection(token0, token1) → (base0[], base1[]) и события NewDVM/NewDPP/NewDSP
//!   (baseToken, quoteToken, creator, pool) — Factory/DVMFactory.sol, DPPFactory.sol, DSPFactory.sol.

alloy::sol! {
    interface IDodoPool {
        event DODOSwap(address fromToken, address toToken, uint256 fromAmount, uint256 toAmount, address trader, address receiver);
        event BuyShares(address to, uint256 increaseShares, uint256 totalShares);
        event SellShares(address payer, address to, uint256 decreaseShares, uint256 totalShares);
        event RChange(uint8 newRState);
        event DODOFlashLoan(address borrower, address assetTo, uint256 baseAmount, uint256 quoteAmount);

        function _BASE_TOKEN_() external view returns (address);
        function _QUOTE_TOKEN_() external view returns (address);
        function getPMMStateForCall() external view returns (uint256 i, uint256 K, uint256 B, uint256 Q, uint256 B0, uint256 Q0, uint256 R);
        function getUserFeeRate(address user) external view returns (uint256 lpFeeRate, uint256 mtFeeRate);
        function getMidPrice() external view returns (uint256 midPrice);
        // querySellBase/querySellQuote: DVM возвращает (receive, mtFee); DPP/DSP добавляют (RState, newTarget).
        // Для сверки нужен только первый uint256 (выход после комиссий) — его читаем из первого слова ответа
        // (quote/quoter_calls.rs decode), независимо от числа возвращаемых значений вида пула.
        function querySellBase(address trader, uint256 payBaseAmount) external view returns (uint256 receiveQuoteAmount, uint256 mtFee);
        function querySellQuote(address trader, uint256 payQuoteAmount) external view returns (uint256 receiveBaseAmount, uint256 mtFee);
    }

    interface IDodoFactory {
        event NewDVM(address baseToken, address quoteToken, address creator, address dvm);
        event NewDPP(address baseToken, address quoteToken, address creator, address dpp);
        event NewDSP(address baseToken, address quoteToken, address creator, address DSP);
        function getDODOPoolBidirection(address token0, address token1) external view returns (address[] baseToken0Pools, address[] baseToken1Pools);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::sol_types::{SolCall, SolEvent};
    use alloy_primitives::{B256, keccak256};

    /// topic0 и селекторы совпадают с keccak256 канонических сигнатур официального контракта (DODOEX/contractV2).
    #[test]
    fn signatures_match_official() {
        let topic = |sig: &str| B256::from(keccak256(sig.as_bytes()));
        assert_eq!(
            IDodoPool::DODOSwap::SIGNATURE_HASH,
            topic("DODOSwap(address,address,uint256,uint256,address,address)")
        );
        assert_eq!(
            IDodoPool::BuyShares::SIGNATURE_HASH,
            topic("BuyShares(address,uint256,uint256)")
        );
        assert_eq!(
            IDodoPool::SellShares::SIGNATURE_HASH,
            topic("SellShares(address,address,uint256,uint256)")
        );
        assert_eq!(IDodoPool::RChange::SIGNATURE_HASH, topic("RChange(uint8)"));
        assert_eq!(
            IDodoPool::DODOFlashLoan::SIGNATURE_HASH,
            topic("DODOFlashLoan(address,address,uint256,uint256)")
        );
        let sel = |sig: &str| keccak256(sig.as_bytes())[..4].to_vec();
        assert_eq!(
            IDodoPool::getPMMStateForCallCall::SELECTOR.to_vec(),
            sel("getPMMStateForCall()")
        );
        assert_eq!(
            IDodoPool::getUserFeeRateCall::SELECTOR.to_vec(),
            sel("getUserFeeRate(address)")
        );
        assert_eq!(
            IDodoPool::querySellBaseCall::SELECTOR.to_vec(),
            sel("querySellBase(address,uint256)")
        );
        assert_eq!(
            IDodoPool::querySellQuoteCall::SELECTOR.to_vec(),
            sel("querySellQuote(address,uint256)")
        );
        assert_eq!(
            IDodoPool::_BASE_TOKEN_Call::SELECTOR.to_vec(),
            sel("_BASE_TOKEN_()")
        );
        assert_eq!(
            IDodoFactory::getDODOPoolBidirectionCall::SELECTOR.to_vec(),
            sel("getDODOPoolBidirection(address,address)")
        );
        assert_eq!(
            IDodoFactory::NewDVM::SIGNATURE_HASH,
            topic("NewDVM(address,address,address,address)")
        );
    }
}
