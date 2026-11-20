//! Файл: venues/bsc_dex/pools/abi_wombat.rs
//! Что делает: решение 484 — ABI пула Wombat Exchange (PoolV2 / HighCovRatioFeePoolV2 / DynamicPoolV2) и его активов
//! (Asset / DynamicAsset): геттеры состояния, квотер quotePotentialSwap, обмен swap, события пула (любое → перечитать
//! пул); слоты хранилища пула — реализация прокси EIP-1967 и PausableAssets._pausedAssets (геттера isPaused у части
//! развёрнутых реализаций нет).
//! Кто вызывает: pools/load_wombat*.rs, verify_wombat.rs, events_wombat.rs, quote/quoter_wombat.rs.
//! От чего зависит: alloy::sol!, alloy-primitives (keccak256).
//! Настройки: нет (пулы — dex.stable_pools, реализации — dex.wombat_implementations, dex.wombat_dynamic_implementations).
//!
//! Источник: wombat-exchange/v1-core 2ac067b8 contracts/wombat-core/pool/PoolV2.sol — события L82-L114, геттеры
//! ampFactor L48, haircutRate L51, getTokens L359, addressOfAsset L412, swap L843 (whenNotPaused, deadline,
//! safeTransferFrom(msg.sender, актив входа) L872, выход — transferUnderlyingToken(to) L873), quotePotentialSwap L888;
//! HighCovRatioFeePoolV2.sol L16-L17 (startCovRatio, endCovRatio); PausableAssets.sol L13, L18 (PausedAsset / UnpausedAsset,
//! address + address); OpenZeppelin PausableUpgradeable — paused(), Paused / Unpaused(address);
//! asset/Asset.sol L22-L36 (underlyingToken, underlyingTokenDecimals, pool, cash, liability), IRelativePriceProvider —
//! getRelativePrice(). Слот _pausedAssets = 201 — storageLayout всех 12 развёрнутых реализаций (Sourcify v2, поле
//! storageLayout; docs/sources/wombat_arbitrum/README.md); ключ — адрес монеты (PoolV2.sol L321-L324 pauseAsset(token)
//! → _pauseAsset(token); swap — requireAssetNotPaused(fromToken) L855). Слот реализации — EIP-1967
//! (eips.ethereum.org/EIPS/eip-1967: keccak256('eip1967.proxy.implementation') − 1; прокси —
//! OptimizedTransparentUpgradeableProxy, Sourcify exact_match).

use alloy_primitives::{Address, B256, U256, keccak256};

alloy::sol! {
    /// Пул Wombat (PoolV2 и наследники).
    interface IWombatPool {
        event AssetAdded(address indexed token, address indexed asset);
        event AssetRemoved(address indexed token, address indexed asset);
        event Deposit(address indexed sender, address token, uint256 amount, uint256 liquidity, address indexed to);
        event Withdraw(address indexed sender, address token, uint256 amount, uint256 liquidity, address indexed to);
        event Swap(address indexed sender, address fromToken, address toToken, uint256 fromAmount, uint256 toAmount, address indexed to);
        event SetAmpFactor(uint256 value);
        event SetHaircutRate(uint256 value);
        event FillPool(address token, uint256 amount);
        event Paused(address account);
        event Unpaused(address account);
        event PausedAsset(address token, address account);
        event UnpausedAsset(address token, address account);

        function getTokens() external view returns (address[] memory);
        function addressOfAsset(address token) external view returns (address);
        function ampFactor() external view returns (uint256);
        function haircutRate() external view returns (uint256);
        function startCovRatio() external view returns (uint128);
        function endCovRatio() external view returns (uint128);
        function paused() external view returns (bool);
        function quotePotentialSwap(address fromToken, address toToken, int256 fromAmount) external view returns (uint256 potentialOutcome, uint256 haircut);
        function swap(address fromToken, address toToken, uint256 fromAmount, uint256 minimumToAmount, address to, uint256 deadline) external returns (uint256 actualToAmount, uint256 haircut);
    }

    /// Актив монеты пула Wombat (Asset; DynamicAsset — ещё getRelativePrice).
    interface IWombatAsset {
        function underlyingToken() external view returns (address);
        function underlyingTokenDecimals() external view returns (uint8);
        function pool() external view returns (address);
        function cash() external view returns (uint120);
        function liability() external view returns (uint120);
        function getRelativePrice() external view returns (uint256);
    }
}

/// Слот PausableAssets._pausedAssets в хранилище пула (Sourcify storageLayout развёрнутых реализаций — см. шапку).
pub const PAUSED_ASSETS_SLOT: u64 = 201;

/// Слот реализации прокси EIP-1967 — тот же расчёт, что у WooFi (abi_woo.rs).
pub use super::abi_woo::implementation_slot;

/// Слот _pausedAssets[монета]: keccak256(abi.encode(монета, 201)) — правило mapping Solidity (docs.soliditylang.org,
/// «Mappings and Dynamic Arrays»). Ошибок нет.
pub fn paused_slot(token: Address) -> B256 {
    let mut buf = [0u8; 64];
    buf[12..32].copy_from_slice(token.as_slice());
    buf[32..64].copy_from_slice(&U256::from(PAUSED_ASSETS_SLOT).to_be_bytes::<32>());
    keccak256(buf)
}
