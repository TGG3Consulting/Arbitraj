//! Файл: venues/bsc_dex/quote/quoter_dmm.rs
//! Что делает: квотер сверки пула KyberSwap Classic DMM (решение 287) — официальный роутер DMMRouter02 своей DEX
//! (dex.quoter): getAmountsOut(amountIn, [пул], [tokenIn, tokenOut]) — тот же getTradeInfo пула и та же формула
//! DMMLibrary.getAmountOut, которыми пользуется сам роутер при обмене; выход — последний элемент amounts.
//! Вынесено из quoter_calls.rs (предел размера файла).
//! Кто вызывает: quote/quoter_calls.rs (request, decode).
//! От чего зависит: pools/abi_dmm.rs, pools/registry.rs, ctx.rs.
//! Настройки: dex.quoter [DEX вида kyber_dmm] — адрес DMMRouter02.
//!
//! Источник: dmm-smart-contracts@e557b57 contracts/periphery/DMMRouter02.sol L625-L632 (verifyPoolsPathSwap — пул из
//! реестра фабрики и токены пути — его пара), libraries/DMMLibrary.sol L113-L139 (getAmountsOut). Адрес BSC —
//! docs.kyberswap.com, Classic Contract Addresses (BSC, Dynamic Fee: DMMRouter 0x78df…6409; Sourcify match).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::PoolRef;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::pools::abi_dmm::IDmmRouter;
use super::super::pools::registry::Entry;

/// Адрес роутера и данные вызова getAmountsOut. Принимает: окружение, запись пула, токен входа, сумму. Ошибки:
/// NotConfigured (dex.quoter), Internal (ссылка на пул не контракт).
pub fn request(
    ctx: &Ctx,
    e: &Entry,
    token_in: Address,
    amount: U256,
) -> GatewayResult<(Address, Bytes)> {
    let PoolRef::Contract(pool) = e.pool.reference else {
        return Err(ctx.err(ErrorCode::Internal, "ссылка на пул DMM не того вида"));
    };
    let token_out = if token_in == e.pool.token0 {
        e.pool.token1
    } else {
        e.pool.token0
    };
    let c = IDmmRouter::getAmountsOutCall {
        amountIn: amount,
        poolsPath: vec![pool],
        path: vec![token_in, token_out],
    };
    Ok((
        ctx.dex_addr("dex.quoter", e.family.id)?,
        Bytes::from(c.abi_encode()),
    ))
}

/// Выход из ответа getAmountsOut — последний элемент. None — ответ не по ABI или пуст. Ошибок нет.
pub fn decode(out: &[u8]) -> Option<U256> {
    IDmmRouter::getAmountsOutCall::abi_decode_returns(out)
        .ok()?
        .last()
        .copied()
}
