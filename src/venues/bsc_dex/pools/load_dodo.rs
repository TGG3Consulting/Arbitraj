//! Файл: venues/bsc_dex/pools/load_dodo.rs
//! Что делает: чтение состояния пула DODO PMM (DVM/DPP/DSP, решения 80, 86b) на заданном блоке: кривая
//! `getPMMStateForCall()` (i, K, B, Q, B0, Q0, R — adjustedTarget уже применён на контракте), комиссии
//! `getUserFeeRate(user)` (lpFeeRate, mtFeeRate) и ориентация base/quote (`_BASE_TOKEN_`/`_QUOTE_TOKEN_`) —
//! собирает PoolState::Pmm. Карты тиков/бинов у PMM нет (кривая задаётся резервами и целями).
//! Читается по одному пулу (как Bin/Elastic): под пачку load_batch.rs не ложится.
//! Кто вызывает: pools/load.rs (и через него load_batch.rs для DODO).
//! От чего зависит: ctx.rs, node/call.rs, pools/abi_dodo.rs, core/types (PoolState).
//! Настройки: нет (адрес пула — из справочника).
//!
//! Геттеры — IDODOV2 (DODOEX/contractV2, решение 23): getPMMStateForCall (DVMStorage.sol L62-L88 и аналоги
//! DPP/DSP), getUserFeeRate (DVMVault.sol L38-L41: lpFeeRate=_LP_FEE_RATE_, mtFeeRate=_MT_FEE_RATE_MODEL_.getFeeRate).
//! Комиссию читаем для user = address(0) (ровно тем же адресом сверяемся квотером querySell* в quote/quoter_calls.rs —
//! важна согласованность локального расчёта и on-chain, а не конкретный трейдер). base_is_token0 = (base < quote):
//! token0 — меньший по адресу (как во всех видах пулов).

use alloy_primitives::{Address, U256};

use crate::core::types::{PmmData, PoolState};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_dodo::IDodoPool;

/// Адрес трейдера для чтения комиссии getUserFeeRate и сверки querySell* — согласованный ноль (см. шапку).
pub const FEE_TRADER: Address = Address::ZERO;

/// Состояние пула DODO PMM на блоке.
/// Принимает: адрес пула, блок-тег.
/// Ошибки: узел, ответ не по ABI, R вне 0..=2.
pub async fn load(ctx: &Ctx, pool: Address, tag: &str) -> GatewayResult<PoolState> {
    let st = call::view(ctx, pool, &IDodoPool::getPMMStateForCallCall {}, tag).await?;
    let fee = call::view(
        ctx,
        pool,
        &IDodoPool::getUserFeeRateCall { user: FEE_TRADER },
        tag,
    )
    .await?;
    let base = call::view(ctx, pool, &IDodoPool::_BASE_TOKEN_Call {}, tag).await?;
    let quote = call::view(ctx, pool, &IDodoPool::_QUOTE_TOKEN_Call {}, tag).await?;
    if st.R > U256::from(2u8) {
        return Err(ctx.err(
            ErrorCode::BadResponse,
            format!("DODO getPMMStateForCall: R = {} вне 0..=2", st.R),
        ));
    }
    let r: u8 = st.R.to::<u8>();
    Ok(PoolState::Pmm(Box::new(PmmData {
        i: st.i,
        k: st.K,
        b: st.B,
        q: st.Q,
        b0: st.B0,
        q0: st.Q0,
        r,
        lp_fee_rate: fee.lpFeeRate,
        mt_fee_rate: fee.mtFeeRate,
        base_is_token0: base < quote,
    })))
}

/// Ориентация base/quote пула DODO по адресам токенов (без состояния): base_is_token0 = (base < quote). Ошибок нет.
pub fn base_is_token0(base: Address, quote: Address) -> bool {
    base < quote
}

/// u8 R-состояния (0/1/2) с контракта → RState математики DODO (PMMPricing.RState: ONE/ABOVE_ONE/BELOW_ONE).
/// None — значение вне 0..=2. Ошибок нет.
pub fn rstate(r: u8) -> Option<super::super::math::swap_dodo::RState> {
    use super::super::math::swap_dodo::RState;
    match r {
        0 => Some(RState::One),
        1 => Some(RState::AboveOne),
        2 => Some(RState::BelowOne),
        _ => None,
    }
}

/// Валовой выход кривой за вычетом комиссий DODO (как querySellBase/querySellQuote, DVMTrader.sol):
/// receive = gross − mulFloor(gross, lpFeeRate) − mulFloor(gross, mtFeeRate). None — переполнение (откат контракта).
pub fn net_after_fees(gross: U256, lp_fee_rate: U256, mt_fee_rate: U256) -> Option<U256> {
    use super::super::math::swap_dodo::mul_floor;
    let lp = mul_floor(gross, lp_fee_rate)?;
    let mt = mul_floor(gross, mt_fee_rate)?;
    gross.checked_sub(lp)?.checked_sub(mt)
}
