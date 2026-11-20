//! Файл: venues/bsc_dex/quote/quoter_fluid.rs
//! Что делает: квотер сверки пула Fluid DEX T1 (решение 440) — официальный резолвер FluidDexReservesResolver
//! (dex.quoter): estimateSwapIn(пул, swap0to1, вход, 0) через eth_call. Резолвер зовёт swapIn пула с получателем
//! ADDRESS_DEAD (пул откатывается с FluidDexSwapResult(выход) до операций со слоем ликвидности) и сам проверяет пределы
//! снятия/займа и скачок цены; не проходит — отвечает 0 (выхода нет). Направление — по token0 самого пула из
//! прочитанного состояния. Вынесено из quoter_calls.rs (предел размера файла).
//! Кто вызывает: quote/quoter_calls.rs (request, decode).
//! От чего зависит: pools/abi_fluid.rs, pools/registry.rs, ctx.rs.
//! Настройки: dex.quoter [DEX вида fluid_t1] — адрес резолвера.
//!
//! Источник: Instadapp/fluid-contracts-public@404d73f — periphery/resolvers/dexReserves/main.sol L734-L903
//! (estimateSwapIn; 0 — пределы или скачок цены), poolT1/coreModule/core/main.sol L338 (FluidDexSwapResult).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::{PoolRef, PoolState};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::pools::abi_fluid::IFluidDexReservesResolver as R;
use super::super::pools::registry::Entry;

/// Адрес резолвера и данные вызова estimateSwapIn. Принимает: окружение, запись пула, токен входа, сумму. Ошибки:
/// Internal (у пула нет состояния SmartColDebt или ссылка не адрес), NotConfigured (dex.quoter).
pub fn request(
    ctx: &Ctx,
    e: &Entry,
    token_in: Address,
    amount: U256,
) -> GatewayResult<(Address, Bytes)> {
    let (PoolState::SmartColDebt(d), PoolRef::Contract(pool)) = (&e.pool.state, &e.pool.reference)
    else {
        return Err(ctx.err(ErrorCode::Internal, "у пула Fluid не прочитано состояние"));
    };
    let resolver = ctx.dex_addr("dex.quoter", e.family.id)?;
    let c = R::estimateSwapInCall {
        dex_: *pool,
        swap0to1_: token_in == d.token0,
        amountIn_: amount,
        amountOutMin_: U256::ZERO,
    };
    Ok((resolver, Bytes::from(c.abi_encode())))
}

/// Выход из ответа estimateSwapIn. None — ответ не по ABI или 0 (обмен откатился бы). Ошибок нет.
pub fn decode(out: &[u8]) -> Option<U256> {
    let v = R::estimateSwapInCall::abi_decode_returns(out).ok()?;
    (!v.is_zero()).then_some(v)
}
