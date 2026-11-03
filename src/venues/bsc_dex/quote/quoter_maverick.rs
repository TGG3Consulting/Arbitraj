//! Файл: venues/bsc_dex/quote/quoter_maverick.rs
//! Что делает: сверка пула Maverick V2 квотером площадки (решение 73; решение 122 F) — данные вызова
//! MaverickV2Quoter.calculateSwap(pool, amount, tokenAIn, exactOutput = false, tickLimit) на точный вход и разбор
//! ответа (amountIn, amountOut, gasEstimate) → amountOut. Квотер исполняет `pool.swap` и откатывается
//! (MaverickV2Quoter.sol L2114-L2134, проверенный исходник; адрес на BSC — docs.mav.xyz v2-contract-addresses).
//! Предел тика — как у шага MAVERICK исполнителя (int32 max для входа tokenA, int32 min для tokenB —
//! contracts/ArbExecutor.sol; IMaverickV2Pool.sol L102-L104): сверяется ровно тот обмен, что пойдёт в сделку. Порт
//! расчёта (math/maverick_swap.rs) за прочитанными тиками даёт неполный расчёт — тогда квотер не зовётся
//! (exec/precheck.rs quoter_now), и бесконечного цикла пула без ликвидности (maverick-v2-examples README L150) нет.
//! Кто вызывает: quote/quoter_calls.rs (request, decode).
//! От чего зависит: pools/abi_maverick.rs, pools/registry.rs, ctx.rs.
//! Настройки: dex.quoter [DEX].

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::{PoolRef, PoolState};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::pools::abi_maverick::IMaverickV2Quoter as Q;
use super::super::pools::registry::Entry;

/// Адрес квотера и данные calculateSwap на точный вход. Принимает: окружение, пул реестра, токен входа, сумму.
/// Ошибки: NotConfigured (dex.quoter), NoRoute (сумма больше uint128), Internal (не тот вид пула).
pub fn request(
    ctx: &Ctx,
    e: &Entry,
    token_in: Address,
    amount: U256,
) -> GatewayResult<(Address, Bytes)> {
    let PoolRef::Contract(pool) = e.pool.reference else {
        return Err(ctx.err(
            ErrorCode::Internal,
            "ссылка на пул Maverick V2 не того вида",
        ));
    };
    let PoolState::TickBin(_) = &e.pool.state else {
        return Err(ctx.err(
            ErrorCode::Internal,
            "состояние пула Maverick V2 не того вида",
        ));
    };
    let amount: u128 = amount
        .try_into()
        .map_err(|_| ctx.err(ErrorCode::NoRoute, "сумма больше uint128"))?;
    let a_in = token_in == e.pool.token0;
    let tick_limit = if a_in { i32::MAX } else { i32::MIN };
    let data = Q::calculateSwapCall {
        pool,
        amount,
        tokenAIn: a_in,
        exactOutput: false,
        tickLimit: tick_limit,
    };
    Ok((
        ctx.dex_addr("dex.quoter", e.family.id)?,
        Bytes::from(data.abi_encode()),
    ))
}

/// Выход из ответа calculateSwap. None — ответ не по ABI. Ошибок нет.
pub fn decode(out: &[u8]) -> Option<U256> {
    Q::calculateSwapCall::abi_decode_returns(out)
        .ok()
        .map(|r| r.amountOut)
}
