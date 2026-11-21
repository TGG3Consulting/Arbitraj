//! Файл: venues/bsc_dex/pools/abi_lb20.rs
//! Что делает: решение 445 — ABI Liquidity Book v2.0 (вид lb_v20; LFJ V2 / Trader Joe V2 на Arbitrum): пул LBPair —
//! поля (фабрика, токены X/Y), состояние (getReservesAndId — активный бин, feeParameters — шаг бина и все параметры
//! комиссии одной структурой, getBin — резервы бина, findFirstNonEmptyBinId — следующий непустой бин), события; роутер
//! LBRouter v2.0 — квотер сверки getSwapOut(пара, вход, swapForY). Реестр фабрики getLBPairInformation — та же подпись и
//! тот же ответ, что у v2.1 (abi_lb.rs ILBFactory).
//! Кто вызывает: pools/verify_lb20.rs, load_lb20.rs, lb_head.rs, events_lb20.rs, settle_read.rs, settle.rs,
//! quote/quoter_lb20.rs, exec/pool_cond_topics.rs.
//! От чего зависит: alloy::sol!.
//! Настройки: нет (адреса — dex.factory, dex.quoter [DEX вида lb_v20]).
//!
//! Источник (решение 23): github.com/lfj-gg/joe-v2, тег v2.0.0 (07f10212e2de6abc97932240c27388c14868582c) —
//! src/interfaces/ILBPair.sol L99-L146 события, L148-L199 геттеры, L206 swap; src/libraries/FeeHelper.sol L29-L44
//! (FeeParameters: 12 полей, все статические — ABI-кортеж из 12 слов); src/LBPair.sol L147-L158 getReservesAndId,
//! L249-L251 feeParameters, L257-L259 findFirstNonEmptyBinId (_tree.findFirstBin — нет бина в эту сторону: откат
//! TreeMath__ErrorDepthSearch, src/libraries/TreeMath.sol L57-L60), L265-L267 getBin (uint256, значения ≤ uint112);
//! src/LBRouter.sol L149-L186 getSwapOut (вход не исполнился целиком — откат LBRouter__TooMuchTokensIn, L185);
//! src/LBErrors.sol L15, L169. getBin(uint24) — тот же селектор, что у v2.1 (abi_lb.rs), ответ — два слова того же вида.

// Код функций и событий генерирует alloy::sol! — число аргументов задаёт ABI контракта, а не мы.
#![allow(clippy::too_many_arguments)]

use alloy::sol_types::SolEvent;
use alloy_primitives::B256;

alloy::sol! {
    interface ILBPairV20 {
        struct FeeParameters {
            uint16 binStep;
            uint16 baseFactor;
            uint16 filterPeriod;
            uint16 decayPeriod;
            uint16 reductionFactor;
            uint24 variableFeeControl;
            uint16 protocolShare;
            uint24 maxVolatilityAccumulated;
            uint24 volatilityAccumulated;
            uint24 volatilityReference;
            uint24 indexRef;
            uint40 time;
        }

        function factory() external view returns (address);
        function tokenX() external view returns (address);
        function tokenY() external view returns (address);
        function getReservesAndId() external view returns (uint256 reserveX, uint256 reserveY, uint256 activeId);
        function feeParameters() external view returns (FeeParameters memory);
        function findFirstNonEmptyBinId(uint24 id_, bool sentTokenY) external view returns (uint24 id);
        function getBin(uint24 id) external view returns (uint256 reserveX, uint256 reserveY);

        event Swap(address indexed sender, address indexed recipient, uint256 indexed id, bool swapForY, uint256 amountIn, uint256 amountOut, uint256 volatilityAccumulated, uint256 fees);
        event FlashLoan(address indexed sender, address indexed receiver, address token, uint256 amount, uint256 fee);
        event CompositionFee(address indexed sender, address indexed recipient, uint256 indexed id, uint256 feesX, uint256 feesY);
        event DepositedToBin(address indexed sender, address indexed recipient, uint256 indexed id, uint256 amountX, uint256 amountY);
        event WithdrawnFromBin(address indexed sender, address indexed recipient, uint256 indexed id, uint256 amountX, uint256 amountY);
        event FeesCollected(address indexed sender, address indexed recipient, uint256 amountX, uint256 amountY);
        event ProtocolFeesCollected(address indexed sender, address indexed recipient, uint256 amountX, uint256 amountY);
        event OracleSizeIncreased(uint256 previousSize, uint256 newSize);

        error TreeMath__ErrorDepthSearch();
    }

    interface ILBRouterV20 {
        function getSwapOut(address pair, uint256 amountIn, bool swapForY) external view returns (uint256 amountOut, uint256 feesIn);
    }
}

/// topic0 событий пула, меняющих резервы бинов или активный бин (ILBPair.sol L99-L140): Swap, CompositionFee,
/// DepositedToBin, WithdrawnFromBin. Комиссии v2.0 лежат вне бинов (pairInformation.feesX / feesY), поэтому FlashLoan,
/// FeesCollected, ProtocolFeesCollected бины не меняют и не слушаются; параметры комиссии меняет фабрика без события
/// пары (setFeesParameters / forceDecay — LBPair.sol L841-L852) — их вид перечитывает каждым блоком (per_block_lb.rs).
/// Ошибок нет.
pub fn topics() -> Vec<B256> {
    vec![
        ILBPairV20::Swap::SIGNATURE_HASH,
        ILBPairV20::CompositionFee::SIGNATURE_HASH,
        ILBPairV20::DepositedToBin::SIGNATURE_HASH,
        ILBPairV20::WithdrawnFromBin::SIGNATURE_HASH,
    ]
}

/// topic0 событий изменения ликвидности (DepositedToBin, WithdrawnFromBin). Ошибок нет.
pub fn liquidity_topics() -> Vec<B256> {
    vec![
        ILBPairV20::DepositedToBin::SIGNATURE_HASH,
        ILBPairV20::WithdrawnFromBin::SIGNATURE_HASH,
    ]
}
