//! Файл: venues/bsc_dex/pools/abi_balancer_v3_buffer.rs
//! Что делает: решение 474 — ABI буферов ERC4626 хранилища Balancer V3: чтение буфера у хранилища
//! (getERC4626BufferAsset, getBufferBalance, getMinimumWrapAmount, areBuffersPaused), превью и базовый токен обёртки
//! ERC4626 (asset, previewDeposit, previewRedeem), события буфера (Wrap, Unwrap, LiquidityAddedToBuffer,
//! LiquidityRemovedFromBuffer — лог от адреса хранилища, topic1 — обёртка; подписка пулов идёт по адресу пула и их не
//! видит, состояние буфера бот перечитывает каждым блоком вместе с пулом, pools/per_block.rs; события пишет скрипт
//! записи), квотер BatchRouter querySwapExactIn с шагами isBuffer.
//! Кто вызывает: pools/load_balancer_v3_buffer.rs, pools/verify_balancer_v3_buffer.rs, quote/quoter_balancer_v3.rs.
//! От чего зависит: alloy::sol!.
//! Настройки: нет (адреса — dex.vault, dex.bv3_batch_router [DEX]).
//!
//! Источник (решение 23): balancer/balancer-deployments@b55dc2b8 build-info v3/tasks/20241204-v3-vault →
//! VaultExtension.sol L663-L665 getERC4626BufferAsset; VaultAdmin.sol L146-L148 getMinimumWrapAmount, L444-L446
//! areBuffersPaused, L764-L767 getBufferBalance (базовый, обёртка — сырые единицы); IVaultEvents.sol L70-L75 Wrap,
//! L84-L89 Unwrap, L196-L201 LiquidityAddedToBuffer, L235-L240 LiquidityRemovedFromBuffer (wrappedToken — indexed);
//! v3/tasks/20241205-v3-batch-router → IBatchRouter.sol L15-L31 (SwapPathStep, SwapPathExactAmountIn), L109-L113
//! querySwapExactIn; BatchRouter.sol L604-L633 (querySwapExactIn: Vault.quote, minAmountOut обнуляется), L193-L205 (шаг
//! isBuffer: direction = UNWRAP, если pool шага = токен входа шага, иначе WRAP). ERC4626 — EIP-4626
//! (https://eips.ethereum.org/EIPS/eip-4626: asset, previewDeposit, previewRedeem).

// Код функций и событий генерирует alloy::sol! — число аргументов задаёт ABI контракта, а не мы.
#![allow(clippy::too_many_arguments)]

alloy::sol! {
    interface IBalancerV3Buffer {
        function getERC4626BufferAsset(address wrappedToken) external view returns (address asset);
        function getBufferBalance(address wrappedToken) external view returns (uint256 underlyingBalanceRaw, uint256 wrappedBalanceRaw);
        function getMinimumWrapAmount() external view returns (uint256 minimumWrapAmount);
        function areBuffersPaused() external view returns (bool buffersPaused);

        event Wrap(address indexed wrappedToken, uint256 depositedUnderlying, uint256 mintedShares, bytes32 bufferBalances);
        event Unwrap(address indexed wrappedToken, uint256 burnedShares, uint256 withdrawnUnderlying, bytes32 bufferBalances);
        event LiquidityAddedToBuffer(address indexed wrappedToken, uint256 amountUnderlying, uint256 amountWrapped, bytes32 bufferBalances);
        event LiquidityRemovedFromBuffer(address indexed wrappedToken, uint256 amountUnderlying, uint256 amountWrapped, bytes32 bufferBalances);
    }

    interface IErc4626 {
        function asset() external view returns (address assetTokenAddress);
        function previewDeposit(uint256 assets) external view returns (uint256 shares);
        function previewRedeem(uint256 shares) external view returns (uint256 assets);
    }

    interface IBalancerV3BatchRouter {
        struct SwapPathStep {
            address pool;
            address tokenOut;
            bool isBuffer;
        }
        struct SwapPathExactAmountIn {
            address tokenIn;
            SwapPathStep[] steps;
            uint256 exactAmountIn;
            uint256 minAmountOut;
        }
        function querySwapExactIn(SwapPathExactAmountIn[] memory paths, address sender, bytes calldata userData) external returns (uint256[] memory pathAmountsOut, address[] memory tokensOut, uint256[] memory amountsOut);
    }
}
