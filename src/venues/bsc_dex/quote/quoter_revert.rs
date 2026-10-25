//! Файл: venues/bsc_dex/quote/quoter_revert.rs
//! Что делает: разбор отката квотера: «у пула не хватает ликвидности на сумму» — не сбой квотера, а свойство пула на
//! эту сумму. Текст причины — читаемый, без hex. Распознаётся:
//! - V4Quoter / CLQuoter / BinQuoter: UnexpectedRevertBytes(bytes) (0x6190b2b0) с вложенной NotEnoughLiquidity(PoolId)
//!   (0x7a5ed734) или BinPool__OutOfLiquidity() (0x96aa65ad);
//! - QuoterV2 (V3): «SPL» — цена пула у границы (require в UniswapV3Pool.swap), «Unexpected error» — откат пула без
//!   причины (QuoterV2.parseRevertReason; так откатывается пул с liquidity() = 0).
//!
//! Кто вызывает: quote/quoter.rs (compare), exec/precheck.rs, check/probe_calls.rs (шаг квотера в пробе налогов),
//! check/hooks_probe.rs (проба крючка: «не хватает ликвидности» с PoolId — short_pool).
//! От чего зависит: gateway/errors.rs (raw — ответ узла), alloy::sol!.
//! Настроек нет.
//!
//! Источники:
//! - UnexpectedRevertBytes(bytes revertData) — Uniswap v4-periphery src/libraries/QuoterRevert.sol;
//!   NotEnoughLiquidity(PoolId poolId) — v4-periphery src/base/BaseV4Quoter.sol (PoolId — bytes32 в ABI);
//!   то же в pancakeswap/infinity-periphery src/libraries/QuoterRevert.sol, src/base/BaseInfinityQuoter.sol;
//! - BinPool__OutOfLiquidity() — pancakeswap/infinity-core src/pool-bin/libraries/BinPool.sol;
//! - 'SPL' — Uniswap/v3-core contracts/UniswapV3Pool.sol (swap, проверка sqrtPriceLimitX96);
//!   'Unexpected error' — Uniswap/v3-periphery contracts/lens/QuoterV2.sol (parseRevertReason).
//!
//! Селекторы сверены с ответами BSC (координатор, Issue #7) — тест в tests/bsc_dex_quoter_revert.rs.

use alloy::sol_types::SolError;
use alloy_primitives::Bytes;

use crate::gateway::errors::{ErrorCode, GatewayError};

alloy::sol! {
    error UnexpectedRevertBytes(bytes revertData);
    error NotEnoughLiquidity(bytes32 poolId);
    error BinPool__OutOfLiquidity();
}

/// Причина «не хватает ликвидности» по откату квотера. None — откат другой (это сбой — показывается как есть).
/// Принимает: ошибку eth_call квотера. Ошибок нет.
pub fn liquidity_short(err: &GatewayError) -> Option<&'static str> {
    if err.code != ErrorCode::SwapReverted {
        return None;
    }
    let data = revert_data(err);
    classify(&err.message, data.as_ref().map(|d| d.as_ref()))
}

/// Разбор отката по тексту и данным (eth_call или шаг eth_simulateV1). None — не «не хватает ликвидности».
/// Ошибок нет.
pub fn classify(message: &str, data: Option<&[u8]>) -> Option<&'static str> {
    if let Some(why) = data.and_then(from_data) {
        return Some(why);
    }
    if message.ends_with("execution reverted: SPL") {
        return Some("цена пула у границы диапазона (откат SPL)");
    }
    if message.ends_with("execution reverted: Unexpected error") {
        return Some("пул откатил обмен без причины (QuoterV2: Unexpected error)");
    }
    None
}

/// Разбор данных отката. None — не «не хватает ликвидности». Ошибок нет.
pub fn from_data(data: &[u8]) -> Option<&'static str> {
    let inner = UnexpectedRevertBytes::abi_decode(data).ok()?.revertData;
    let sel = inner.get(..4)?;
    if sel == NotEnoughLiquidity::SELECTOR {
        Some("квотер: NotEnoughLiquidity")
    } else if sel == BinPool__OutOfLiquidity::SELECTOR {
        Some("квотер: BinPool__OutOfLiquidity")
    } else {
        None
    }
}

/// PoolId из отката «не хватает ликвидности»: UnexpectedRevertBytes(NotEnoughLiquidity(poolId)) — квотеры V4 и
/// Infinity CL (v4-periphery src/base/BaseV4Quoter.sol, infinity-periphery src/base/BaseInfinityQuoter.sol;
/// UnexpectedRevertBytes — src/libraries/QuoterRevert.sol обоих). Принимает: ошибку eth_call квотера. None — откат
/// другой. Ошибок нет.
pub fn short_pool(err: &GatewayError) -> Option<alloy_primitives::B256> {
    if err.code != ErrorCode::SwapReverted {
        return None;
    }
    short_pool_id(revert_data(err)?.as_ref())
}

/// PoolId из данных отката UnexpectedRevertBytes(NotEnoughLiquidity(poolId)). None — данные другие. Ошибок нет.
pub fn short_pool_id(data: &[u8]) -> Option<alloy_primitives::B256> {
    let inner = UnexpectedRevertBytes::abi_decode(data).ok()?.revertData;
    NotEnoughLiquidity::abi_decode(&inner)
        .ok()
        .map(|e| e.poolId)
}

/// Данные отката из ответа узла: error.data (hex). None — нет. Ошибок нет.
fn revert_data(err: &GatewayError) -> Option<Bytes> {
    let v: serde_json::Value = serde_json::from_str(err.raw.as_deref()?).ok()?;
    v.get("error")?.get("data")?.as_str()?.parse().ok()
}
