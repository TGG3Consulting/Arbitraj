//! Файл: venues/bsc_dex/quote/quoter_crypto.rs
//! Что делает: квотер сверки crypto-пула Curve (решение 442: Twocrypto-NG, Tricrypto-NG) — сам пул: get_dy(i, j, dx)
//! через eth_call (view; пул зовёт свой контракт представлений Views с той же математикой MATH на том же блоке).
//! Индексы монет пары — из прочитанного состояния (i0 — token0). Вынесено из quoter_calls.rs (предел размера файла).
//! Кто вызывает: quote/quoter_calls.rs (request, decode).
//! От чего зависит: pools/abi_crypto.rs, pools/registry.rs, ctx.rs.
//! Настройки: нет (адрес — сам пул).
//!
//! Источник: curvefi/twocrypto-ng 89c8dfb CurveTwocryptoOptimized.vy get_dy L1522-L1534 (→ Views
//! CurveCryptoViews2Optimized.vy get_dy L63-L76); curvefi/tricrypto-ng ecaa816 CurveTricryptoOptimizedWETH.vy get_dy
//! L1669-L1681. Views считает выход так же, как `_exchange` (в рампе — тот же пересчёт D через newton_D,
//! CurveCryptoViews2Optimized.vy `_calc_D_ramp` L161-L178; решение 453 — наша котировка делает то же).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::PoolState;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::pools::abi_crypto::ICryptoPool as P;
use super::super::pools::registry::Entry;

/// Адрес пула и данные вызова get_dy. Принимает: окружение, запись пула, токен входа, сумму. Ошибки: Internal (у пула
/// нет состояния Crypto или ссылка не свой контракт пула).
pub fn request(
    ctx: &Ctx,
    e: &Entry,
    token_in: Address,
    amount: U256,
) -> GatewayResult<(Address, Bytes)> {
    // Решение 295: и пара многомонетного пула (PoolRef::Multi) — квотер тот же контракт, индексы — из состояния пары.
    let (PoolState::Crypto(d), Some(pool)) = (&e.pool.state, e.pool.reference.own_contract())
    else {
        return Err(ctx.err(
            ErrorCode::Internal,
            "у crypto-пула Curve не прочитано состояние (индексы монет)",
        ));
    };
    let (i, j) = if token_in == e.pool.token0 {
        (d.i0, d.i1)
    } else {
        (d.i1, d.i0)
    };
    let c = P::get_dyCall {
        i: U256::from(i),
        j: U256::from(j),
        dx: amount,
    };
    Ok((pool, Bytes::from(c.abi_encode())))
}

/// Выход из ответа get_dy. None — ответ не по ABI. Ошибок нет.
pub fn decode(out: &[u8]) -> Option<U256> {
    P::get_dyCall::abi_decode_returns(out).ok()
}
