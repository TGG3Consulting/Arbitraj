//! Файл: venues/bsc_dex/quote/native_gas.rs
//! Что делает: затрата WrapGas котировки нативной монеты BNB (решение 62): газ WRAP_ETH (продажа) или UNWRAP_WETH
//! (покупка) в той же транзакции сверх газа обмена токеном WBNB — замер пробы нативной монеты (check/native_probe.rs)
//! в пуле стороны BNB каждой доли (продажа — первый пул, покупка — последний); обёртка одна на транзакцию — берётся
//! наибольший замер долей. Не измерено — котировки нет («не торгуем вслепую»).
//! Кто вызывает: quote/mod.rs.
//! От чего зависит: check/state.rs, exec/native.rs, pools/registry.rs, num.rs, core/types.
//! Настройки: через exec/native.rs — dex.native_ticker, dex.native_wrapped [bsc_dex].

use alloy_primitives::U256;

use crate::core::types::{Cost, CostKind, Quote, Side};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::exec::native;
use super::super::num;
use super::super::wallet::balances::BNB;
use super::Quoting;

/// Затрата газа обёртки и её единицы газа. Принимает: котировку (маршрут), цену газа (wei).
/// Возвращает: None — монета не нативная. Ошибки: CoinNotChecked (газ обёртки не измерен или проба не прошла — причина),
/// Internal (нет маршрута), BadResponse (число).
pub fn cost(q: &Quoting, quote: &Quote, gas_wei: U256) -> GatewayResult<Option<(Cost, u64)>> {
    let ctx = &q.ctx;
    if !native::is_native(ctx, &quote.asset) {
        return Ok(None);
    }
    let route = quote
        .route
        .as_ref()
        .ok_or_else(|| ctx.err(ErrorCode::Internal, "у котировки нет маршрута"))?;
    let mut units = 0u64;
    for leg in &route.legs {
        let hop = match quote.side {
            Side::Sell => leg.hops.first(),
            Side::Buy => leg.hops.last(),
        }
        .ok_or_else(|| ctx.err(ErrorCode::Internal, "в доле нет пула"))?;
        let g = match q.checks.pool(&hop.pool).wrap_gas {
            Some(Ok((buy, sell))) => match quote.side {
                Side::Buy => buy,
                Side::Sell => sell,
            },
            Some(Err(why)) => {
                return Err(ctx.err(
                    ErrorCode::CoinNotChecked,
                    format!(
                        "{}: проба WRAP/UNWRAP не прошла — {why}",
                        quote.asset.ticker
                    ),
                ));
            }
            None => {
                return Err(ctx.err(
                    ErrorCode::CoinNotChecked,
                    format!(
                        "{}: газ обёртки (WRAP/UNWRAP) ещё не измерен",
                        quote.asset.ticker
                    ),
                ));
            }
        };
        units = units.max(g);
    }
    let wei = gas_wei.saturating_mul(U256::from(units));
    let bnb = num::to_decimal(wei, num::BNB_DECIMALS).ok_or_else(|| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("Газ обёртки {wei} wei не переводится в BNB"),
        )
    })?;
    Ok(Some((Cost::new(CostKind::WrapGas, bnb, BNB, false), units)))
}
