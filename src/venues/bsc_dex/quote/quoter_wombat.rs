//! Файл: venues/bsc_dex/quote/quoter_wombat.rs
//! Что делает: решение 484 — квотер сверки пула Wombat Exchange — сам пул: quotePotentialSwap(fromToken, toToken,
//! int256 вход) (тот же _quoteFrom, что у swap; ответ — (potentialOutcome, haircut), выход — первое слово). Отличия от
//! swap (их котировка бота учитывает сама, math/wombat.rs swap_out): квотер не проверяет паузу пула и актива входа,
//! addCash / removeCash и покрытие выхода ≥ 1 %. Вынесено из quoter_calls.rs (предел размера файла).
//! Кто вызывает: quote/quoter_calls.rs (request, decode).
//! От чего зависит: pools/abi_wombat.rs, pools/registry.rs, ctx.rs.
//! Настройки: нет (адрес — сам пул).
//!
//! Источник: wombat-exchange/v1-core 2ac067b8 contracts/wombat-core/pool/PoolV2.sol L888-L907 (quotePotentialSwap).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, I256, U256};

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::pools::abi_wombat::IWombatPool;
use super::super::pools::registry::Entry;

/// Адрес пула и данные вызова quotePotentialSwap. Принимает: окружение, запись пула, токен входа, сумму. Ошибки:
/// Internal (ссылка на пул без своего контракта), NoRoute (сумма ≥ 2^255 — не int256).
pub fn request(
    ctx: &Ctx,
    e: &Entry,
    token_in: Address,
    amount: U256,
) -> GatewayResult<(Address, Bytes)> {
    let Some(pool) = e.pool.reference.own_contract() else {
        return Err(ctx.err(ErrorCode::Internal, "ссылка на пул Wombat не того вида"));
    };
    let token_out = if token_in == e.pool.token0 {
        e.pool.token1
    } else {
        e.pool.token0
    };
    let from_amount = I256::try_from(amount)
        .map_err(|_| ctx.err(ErrorCode::NoRoute, "сумма квотера Wombat не помещается в int256"))?;
    let c = IWombatPool::quotePotentialSwapCall {
        fromToken: token_in,
        toToken: token_out,
        fromAmount: from_amount,
    };
    Ok((pool, Bytes::from(c.abi_encode())))
}

/// Выход из ответа quotePotentialSwap — potentialOutcome. None — ответ не по ABI. Ошибок нет.
pub fn decode(out: &[u8]) -> Option<U256> {
    IWombatPool::quotePotentialSwapCall::abi_decode_returns(out)
        .ok()
        .map(|r| r.potentialOutcome)
}
