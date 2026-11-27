//! Формат CrocSwap-protocol, commit d65b20f. Те же пределы и пути использует AmbientWallet.sol.
use alloy_primitives::U256;

// Фиксированный массив адресов proxyPaths_ начинается после упакованного слота 0.
// https://github.com/CrocSwap/CrocSwap-protocol/blob/d65b20f282e6ef7f52fe999601cd56098cde623e/contracts/mixins/StorageLayout.sol#L50-L59
const PROXY_PATHS_SLOT: u64 = 1;
// https://github.com/CrocSwap/CrocSwap-protocol/blob/d65b20f282e6ef7f52fe999601cd56098cde623e/contracts/mixins/StorageLayout.sol#L186-L195
pub(super) const SWAP_CALLPATH: u16 = 1;
pub(super) const FLAG_CROSS_CALLPATH: u16 = 3500;

pub(super) fn proxy_slot(path: u16) -> U256 {
    U256::from(PROXY_PATHS_SLOT + u64::from(path))
}

// Нижняя граница включительна, верхняя исключительна.
// https://github.com/CrocSwap/CrocSwap-protocol/blob/d65b20f282e6ef7f52fe999601cd56098cde623e/contracts/libraries/TickMath.sol#L14-L16
// https://github.com/CrocSwap/CrocSwap-protocol/blob/d65b20f282e6ef7f52fe999601cd56098cde623e/contracts/libraries/TickMath.sol#L68
pub(super) const MIN_LIMIT_PRICE: u128 = 65538;
pub(super) const MAX_LIMIT_PRICE: u128 = 21267430153580247136652501917186561138 - 1;

#[cfg(test)]
#[path = "ambient_format_tests.rs"]
mod tests;
