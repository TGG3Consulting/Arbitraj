//! Файл: venues/bsc_dex/pools/abi_izi.rs
//! Что делает: ABI пулов iZiSwap и Biswap V3 (решение 286; виды iziswap, biswap_v3) — только вызовы, которые бот
//! делает: геттеры пула tokenX / tokenY / fee / factory (проверка — verify_izi.rs), реестр фабрики pool(x, y, fee)
//! (у iZiSwap fee — uint24, у Biswap V3 — uint16: селекторы разные, чужой — откат), topic0 событий пула
//! (events_izi.rs). Обмен (swapX2Y / swapY2X) и обратные вызовы — в контрактах (contracts/IPools.sol, VirtualQuoter.sol).
//! Кто вызывает: pools/verify_izi.rs, pools/load_izi.rs, pools/events_izi.rs.
//! От чего зависит: alloy::sol!, families.rs (Proto).
//! Настройки: нет (фабрика — dex.factory [DEX]).
//!
//! Источники (решение 23): iZiSwap — github.com/izumiFinance/iZiSwap-core@3fe21eba: contracts/iZiSwapPool.sol (поля
//! factory/tokenX/tokenY/fee — public, swap* L383-L438), iZiSwapFactory.sol (pool — mapping(tokenX ⇒ tokenY ⇒ uint24 fee),
//! newPool L95-L126 сортирует пару и пишет оба порядка), interfaces/IiZiSwapPool.sol (события L14-L139). Biswap V3 —
//! Sourcify chain 56: BiswapPoolV3 0x76008e4007ab26142dbeafee24d6113de47eb76b (exact_match; BiswapPoolV3.sol L52-L59 —
//! factory, tokenX, tokenY, uint16 fee; события — IBiswapPoolV3.sol), BiswapFactoryV3 0x7c3d53606f9c03e7f54abddffc3868e1c5466863
//! (match; BiswapFactoryV3.sol L25 — pool: mapping(tokenX ⇒ tokenY ⇒ uint16 fee), newPool L110-L142). Совпадение с
//! развёрнутым кодом трёх фабрик — фактом цепи (tests/data/izi_2026-11-15/probe.json): PUSH4 селекторов в коде,
//! ответ реестра = пул у всех записанных пулов, topic0 Swap — в записанных логах.

// Код функций генерирует alloy::sol! — число полей задаёт ABI контракта, а не мы.
#![allow(clippy::too_many_arguments)]

use alloy_primitives::{B256, b256};

use super::families::Proto;

alloy::sol! {
    interface IIziPool {
        function tokenX() external view returns (address);
        function tokenY() external view returns (address);
        function fee() external view returns (uint24);
        function factory() external view returns (address);
    }

    /// Biswap V3: fee пула — uint16 (BiswapPoolV3.sol L59; ABI-слово то же, что у uint24).
    interface IBiswapPoolV3 {
        function fee() external view returns (uint16);
    }

    interface IIziFactory {
        function pool(address tokenX, address tokenY, uint24 fee) external view returns (address);
    }

    interface IBiswapFactoryV3 {
        function pool(address tokenX, address tokenY, uint16 fee) external view returns (address);
    }

    /// state() пула — бот берёт только первое слово (sqrtPrice_96, libraries/State.sol у обоих видов): остальные поля
    /// у iZiSwap и Biswap V3 разные (у Biswap — ещё feeTimesL и fee), разбор первого слова — load_izi.rs.
    interface IIziState {
        function state() external view returns (uint160 sqrtPrice_96);
    }
}

/// topic0 событий пула iZiSwap (IiZiSwapPool.sol L14-L139: Mint, Burn, CollectLiquidity, Swap, Flash, AddLimitOrder,
/// DecLimitOrder, CollectLimitOrder; Swap 0x0fe977d6… — в записанных логах пулов iziswap-bsc). Ошибок нет.
fn izi_topics() -> Vec<B256> {
    vec![
        // Swap(address,address,uint24,bool,uint256,uint256,int24)
        b256!("0x0fe977d619f8172f7fdbe8bb8928ef80952817d96936509f67d66346bc4cd10f"),
        // Mint(address,address,int24,int24,uint128,uint256,uint256)
        b256!("0x7a53080ba414158be7ec69b987b5fb7d07dee101fe85488f0853ae16239d0bde"),
        // Burn(address,int24,int24,uint128,uint256,uint256)
        b256!("0x0c396cd989a39f4459b5fa1aed6a9a8dcdbc45908acfd67e028cd568da98982c"),
        // AddLimitOrder(address,uint128,uint128,int24,uint128,uint128,bool)
        b256!("0x4f4658280ee6d0e8f09b5e436dacaca69ec5dd7c2ba05fb010d5145a3567cdad"),
        // DecLimitOrder(address,uint128,int24,uint128,uint128,bool)
        b256!("0x3736ba81d13006f6ea2012ba3e287f087169b55d90a9defb5966fe9eb830d7ea"),
        // Flash(address,address,uint256,uint256,uint256,uint256)
        b256!("0xbdbdb71d7860376ba52b25a5028beea23581364a40522f6bcfb86bb1f2dca633"),
    ]
}

/// topic0 событий пула Biswap V3 (IBiswapPoolV3.sol: Swap(RetVal, StateEvent, bool) — 0xde449b42… в записанных логах;
/// Mint / Burn / ChangeFeeVote со StateEvent, AddLimitOrder, DecLimitOrder, Flash — PUSH32 в коде пула biswap-v3-1).
/// ChangeLiquidityState (тип Data в интерфейсе неоднозначен, в коде пула topic0 нет) не слушается — вид и так
/// перечитывается каждым блоком (per_block.rs). Ошибок нет.
fn biswap_topics() -> Vec<B256> {
    vec![
        // Swap((uint256,uint256,uint128),(int24,uint16,uint128,uint128),bool)
        b256!("0xde449b421e7f751324933a2c4afee2ea35f7c7d2b6bdf310e7a7017b4d67bb91"),
        // Mint(address,address,int24,int24,uint128,uint256,uint256,(int24,uint16,uint128,uint128))
        b256!("0xa8f11e3c83e07326a97320ce22cf2ceb3b05f57c1725a033a120df8fcf4e0e6d"),
        // Burn(address,int24,int24,uint128,uint256,uint256,(int24,uint16,uint128,uint128))
        b256!("0x8d8f4b3f9918ece6e62b590c124d5a5747a92bcf22f7e4628896d62ce813fcca"),
        // ChangeFeeVote(address,int24,int24,uint16,(int24,uint16,uint128,uint128)) — меняет комиссию пула
        b256!("0xa3597de0ff4d26e18395b75a61ac66e894bc1e5a49a628b873775afe0e9aaaf4"),
        // AddLimitOrder(address,uint128,uint128,int24,uint128,uint128,bool)
        b256!("0x4f4658280ee6d0e8f09b5e436dacaca69ec5dd7c2ba05fb010d5145a3567cdad"),
        // DecLimitOrder(address,uint128,int24,uint128,uint128,bool)
        b256!("0x3736ba81d13006f6ea2012ba3e287f087169b55d90a9defb5966fe9eb830d7ea"),
        // Flash(address,address,uint256,uint256,uint256,uint256)
        b256!("0xbdbdb71d7860376ba52b25a5028beea23581364a40522f6bcfb86bb1f2dca633"),
    ]
}

/// Все topic0 пулов вида (любое → перечитать пул). Принимает: протокол вида. Ошибок нет (не iZi — пусто).
pub fn topics(p: Proto) -> Vec<B256> {
    match p {
        Proto::IziSwap => izi_topics(),
        Proto::BiswapV3 => biswap_topics(),
        _ => Vec::new(),
    }
}

/// topic0 Swap обоих видов и Flash (одна подпись у обоих) — события, не меняющие ликвидность и комиссию.
const IZI_SWAP: B256 = b256!("0x0fe977d619f8172f7fdbe8bb8928ef80952817d96936509f67d66346bc4cd10f");
const BISWAP_SWAP: B256 =
    b256!("0xde449b421e7f751324933a2c4afee2ea35f7c7d2b6bdf310e7a7017b4d67bb91");
const FLASH: B256 = b256!("0xbdbdb71d7860376ba52b25a5028beea23581364a40522f6bcfb86bb1f2dca633");

/// topic0 изменения ликвидности и комиссии (мелкие пулы, pools/group.rs Topics::Liquidity): всё, кроме Swap и Flash
/// (отсекаются по значению topic0, ревью t118). Ошибок нет.
pub fn liquidity_topics(p: Proto) -> Vec<B256> {
    topics(p)
        .into_iter()
        .filter(|t| ![IZI_SWAP, BISWAP_SWAP, FLASH].contains(t))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::sol_types::SolCall;

    /// Селекторы = найденным в коде фабрик и пулов BSC (tests/data/izi_2026-11-15/probe.json).
    #[test]
    fn selectors_match_chain() {
        assert_eq!(IIziPool::tokenXCall::SELECTOR, [0x16, 0xdc, 0x16, 0x5b]);
        assert_eq!(IIziPool::tokenYCall::SELECTOR, [0xb7, 0xd1, 0x9f, 0xc4]);
        assert_eq!(IIziPool::feeCall::SELECTOR, [0xdd, 0xca, 0x3f, 0x43]);
        assert_eq!(IIziPool::factoryCall::SELECTOR, [0xc4, 0x5a, 0x01, 0x55]);
        assert_eq!(IIziState::stateCall::SELECTOR, [0xc1, 0x9d, 0x93, 0xfb]);
        assert_eq!(IIziFactory::poolCall::SELECTOR, [0xbe, 0xcb, 0xcc, 0x6a]);
        assert_eq!(
            IBiswapFactoryV3::poolCall::SELECTOR,
            [0x06, 0x68, 0xc8, 0xa2]
        );
        assert_eq!(liquidity_topics(Proto::IziSwap).len(), 4);
        assert_eq!(liquidity_topics(Proto::BiswapV3).len(), 5);
    }
}
