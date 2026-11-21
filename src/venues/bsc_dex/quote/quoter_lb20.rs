//! Файл: venues/bsc_dex/quote/quoter_lb20.rs
//! Что делает: решение 445 — квотер сверки пула Liquidity Book v2.0 (LFJ V2, Arbitrum): LBRouter v2.0
//! getSwapOut(пара, вход, swapForY) через eth_call (view; тот же цикл по бинам, getAmounts и комиссия, что у swap пары
//! v2.0, на том же блоке). У пары v2.0 своего getSwapOut нет (у v2.1 квотер — сам пул, quoter_lb.rs). Ответ (amountOut,
//! feesIn); вход не исполнился целиком — роутер откатывается (LBRouter__TooMuchTokensIn), выхода нет. Направление — по
//! порядку X/Y пула из прочитанного состояния (x_is_token0).
//! Кто вызывает: quote/quoter_calls.rs (request, decode).
//! От чего зависит: pools/abi_lb20.rs, pools/registry.rs, ctx.rs.
//! Настройки: dex.quoter [DEX вида lb_v20] — адрес LBRouter v2.0.
//!
//! Источник: lfj-gg/joe-v2, тег v2.0.0 (07f10212) — src/LBRouter.sol L149-L186 (getSwapOut), L172 (вход в бин больше
//! uint112 — откат LBRouter__BinReserveOverflows), L185 (остаток входа — откат). Адрес LBRouter v2.0 Arbitrum One —
//! официальная документация LFJ (traderjoe-xyz/joe-docs 61c16665 versioned_docs/version-V2.2/deployment-addresses/
//! arbitrum.md L52-L58) и SDK (lfj-gg/joe-sdk-v2 a6f5a8c3 src/constants/v2Addrs.ts L34-L41).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::{PoolRef, PoolState};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::pools::abi_lb20::ILBRouterV20 as R;
use super::super::pools::registry::Entry;

/// Адрес роутера (dex.quoter) и данные вызова getSwapOut. Принимает: окружение, запись пула, токен входа, сумму.
/// Ошибки: Internal (у пула нет состояния LbBin или ссылка не адрес), NotConfigured (dex.quoter).
pub fn request(
    ctx: &Ctx,
    e: &Entry,
    token_in: Address,
    amount: U256,
) -> GatewayResult<(Address, Bytes)> {
    let (PoolState::LbBin(d), PoolRef::Contract(pool)) = (&e.pool.state, &e.pool.reference) else {
        return Err(ctx.err(
            ErrorCode::Internal,
            "у пула Liquidity Book v2.0 не прочитано состояние (порядок X/Y)",
        ));
    };
    let router = ctx.dex_addr("dex.quoter", e.family.id)?;
    let c = R::getSwapOutCall {
        pair: *pool,
        amountIn: amount,
        swapForY: (token_in == e.pool.token0) == d.x_is_token0,
    };
    Ok((router, Bytes::from(c.abi_encode())))
}

/// Выход из ответа getSwapOut. None — ответ не по ABI. Ошибок нет.
pub fn decode(out: &[u8]) -> Option<U256> {
    R::getSwapOutCall::abi_decode_returns(out)
        .ok()
        .map(|r| r.amountOut)
}
