//! Отдельная плата рынка: часть площадок удерживает её внутри самой денежной котировки.
use super::super::{ctx::Ctx, pools::registry::Entry};
use crate::{
    core::types::{PoolKind, PoolState},
    gateway::errors::{ErrorCode, GatewayResult},
};
use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;
pub(super) fn by_volume(
    ctx: &Ctx,
    e: &Entry,
    token_in: Address,
    input: U256,
) -> Option<GatewayResult<Decimal>> {
    if let PoolState::Surge(s)=&e.pool.state {
        let multiplier=if token_in==s.wrapped{s.buy_mul}else{s.sell_mul};
        return Some(Ok(Decimal::from(100u8-multiplier)));
    }
    if let PoolState::Dagrow(s)=&e.pool.state {
        let side=usize::from(token_in==s.dagr);
        return Some(s.fees[side].map(|n|Decimal::new(i64::from(n),2)).ok_or_else(||ctx.err(ErrorCode::NoRoute,"DAGrow: направление остановлено")));
    }
    if let PoolState::Temple(state) = &e.pool.state {
        return Some(
            super::super::math::temple::is_defense(state, token_in, input)
                .map(|d| if d { Decimal::ZERO } else { Decimal::new(5, 1) })
                .ok_or_else(|| {
                    ctx.err(
                        ErrorCode::NoRoute,
                        "TempleDAO: не установлен способ расчёта по этому объёму",
                    )
                }),
        );
    }
    None
}
pub(super) fn rate(ctx: &Ctx, e: &Entry) -> Option<GatewayResult<Decimal>> {
    if e.pool.kind != PoolKind::V2 {
        return None;
    }
    match &e.pool.state {
        PoolState::UmbraeDamm(s) => Some(
            super::super::math::umbrae_damm::rate(s)
                .map(|bps| Decimal::new(i64::from(bps), 2))
                .ok_or_else(|| {
                    ctx.err(
                        ErrorCode::BadResponse,
                        "Umbrae DAMM: неверная ставка комиссии",
                    )
                }),
        ),
        PoolState::YieldBasisLev(s)=>Some(super::super::num::to_decimal(s.fee,16).ok_or_else(||ctx.err(ErrorCode::BadResponse,"YieldBasis: неверная ставка комиссии"))),
        PoolState::BancorLegacy(s)=>{
            let first=Decimal::new(i64::from(s.fee_ppm),4);
            let fee=s.second.as_ref().map_or(first,|b|first+(Decimal::ONE_HUNDRED-first)*Decimal::new(i64::from(b.fee_ppm),6));
            Some(Ok(fee))
        }
        PoolState::YieldBasisPool(_)=>Some(Ok(Decimal::ZERO)),
        PoolState::BalancerV1(s)=>Some(super::super::num::to_decimal(s.fee,16).ok_or_else(||ctx.err(ErrorCode::BadResponse,"Balancer V1: неверная ставка комиссии"))),
        PoolState::BancorV3(s)=>Some(Ok(s.markets.iter().flatten().fold(Decimal::ZERO,|fee,m|fee+(Decimal::ONE_HUNDRED-fee)*Decimal::new(i64::from(m.fee_ppm),6)))),
        PoolState::UniswapV1(s)=>Some(Ok(if s.dual(){Decimal::new(5991,4)}else{Decimal::new(3,1)})),
        PoolState::Antfarm(_)
        | PoolState::Surge(_)
        | PoolState::Dagrow(_)
        | PoolState::Temple(_)
        | PoolState::Rubicon(_)
        | PoolState::Carbon(_)
        | PoolState::MintClub(_)
        | PoolState::Virtuals(_)
        | PoolState::EtherVista(_) => Some(Ok(Decimal::ZERO)),
        _ => None,
    }
}
