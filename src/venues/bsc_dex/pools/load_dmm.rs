//! Файл: venues/bsc_dex/pools/load_dmm.rs
//! Что делает: решение 287 — чтение состояния пула KyberSwap Classic DMM на блоке одним вызовом getTradeInfo():
//! реальные резервы, виртуальные резервы (у пула без усиления пул сам отдаёт реальные) и комиссия ЭТОГО блока
//! (feeInPrecision над 10^18: rFactor по block.number — меняется каждым блоком без событий пула, поэтому вид
//! перечитывается каждым блоком, per_block.rs). Собирает PoolState::AmpV2.
//! Кто вызывает: pools/load.rs (ветка Proto::KyberDmm).
//! От чего зависит: ctx.rs, node/call.rs, abi_dmm.rs, math/dmm.rs (precision).
//! Настройки: нет.
//!
//! Источник — DMMPool.sol@e557b57 getTradeInfo L240-L265 (при ampBps = BPS виртуальные = реальные, комиссия —
//! getFinalFee(FeeFomula.getFee(getRFactor(block.number)), ampBps)); совпадение с кодом BSC — шапка abi_dmm.rs.

use alloy_primitives::{Address, U256};

use crate::core::types::{AmpV2Data, PoolState};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::math::dmm::precision;
use super::super::node::call;
use super::abi_dmm::IDmmPool;

/// Состояние пула на блоке. Принимает: окружение, адрес пула, блок-тег. Ошибки: узел, вызов откатился, ответ не по ABI,
/// комиссия больше 100 % (BadResponse).
pub async fn load(ctx: &Ctx, pool: Address, tag: &str) -> GatewayResult<PoolState> {
    let r = call::view(ctx, pool, &IDmmPool::getTradeInfoCall {}, tag).await?;
    state(
        [r._reserve0, r._reserve1, r._vReserve0, r._vReserve1].map(U256::from),
        r.feeInPrecision,
    )
    .map_err(|w| ctx.err(ErrorCode::BadResponse, format!("KyberSwap DMM {pool}: {w}")))
}

/// Сборка состояния из ответа getTradeInfo (без сети; её же зовут тесты на записанных ответах). Принимает: [резерв0,
/// резерв1, вирт. резерв0, вирт. резерв1], комиссию над 10^18. Возвращает: AmpV2. Ошибка — комиссия больше 100 %.
pub fn state(r: [U256; 4], fee: U256) -> Result<PoolState, &'static str> {
    if fee > precision() {
        return Err("feeInPrecision больше 10^18");
    }
    Ok(PoolState::AmpV2(Box::new(AmpV2Data {
        reserve0: r[0],
        reserve1: r[1],
        v_reserve0: r[2],
        v_reserve1: r[3],
        fee,
    })))
}
