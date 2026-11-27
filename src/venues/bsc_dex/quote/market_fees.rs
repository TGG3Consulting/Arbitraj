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
        PoolState::Antfarm(_)
        | PoolState::Dagrow(_)
        | PoolState::Temple(_)
        | PoolState::Carbon(_)
        | PoolState::MintClub(_)
        | PoolState::Virtuals(_)
        | PoolState::EtherVista(_) => Some(Ok(Decimal::ZERO)),
        _ => None,
    }
}
