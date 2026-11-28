//! Уточняет покупку через legacy Spectra на входах, передаваемых исполнителем.
//! route_calc::buy вызывает после обратного подбора цены. Использует уже прочитанные
//! состояния и измерения крючков. В V5 последующие шаги получают ALL_BPS остатка.

use super::{Ctx, Leg, Swapped, bad, keep, sell_step};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::venues::bsc_dex::{math::full_math, pools::families::Proto};
use alloy_primitives::U256;
use rust_decimal::Decimal;

/// Признак направленного ребра с целочисленной, локально убывающей функцией get_dy.
pub(super) fn legacy_spectra(leg: &Leg) -> bool {
    leg.entry.family.variant.spectra() && leg.entry.family.proto == Proto::CurveCryptoV1
}

/// Воспроизводит прямые выплаты для уже выбранного бюджета. У обычного обратного
/// шага (например, V2) amount_out может быть лишь запрошенным числом. У Spectra
/// обратный шаг уже содержит фактический get_dy на выбранном amount_in.
/// Ошибки: неполный обмен, неподтверждённый вход или недоступный расчёт шага.
pub(super) fn reconcile(
    ctx: &Ctx,
    legs: &[Leg],
    swaps: &mut [Swapped],
    pcts: &mut [Decimal],
) -> GatewayResult<()> {
    let mut amount = swaps[0].amount_in;
    for (i, leg) in legs.iter().enumerate() {
        if amount != swaps[i].amount_in || !leg.entry.family.variant.spectra() {
            let (pct, actual) = sell_step(ctx, leg, amount)?;
            if !actual.complete || actual.amount_in > amount {
                return Err(ctx.err(
                    ErrorCode::NoRoute,
                    "шаг покупки не исполним на переданном остатке",
                ));
            }
            swaps[i] = actual;
            pcts[i] = pct;
        }
        if i + 1 < legs.len() {
            amount = swaps[i].amount_out;
            if !pcts[i].is_zero() {
                let k = keep(pcts[i]);
                if k <= Decimal::ZERO {
                    return Err(ctx.err(
                        ErrorCode::NoRoute,
                        "крючок не оставляет средств для следующего шага",
                    ));
                }
                // Один floor в атомах. Перевод исходного U256 в 96-битную мантиссу
                // Decimal перед умножением мог бы сам создать расхождение на соседних входах.
                let scale = U256::from(10u128.pow(k.scale()));
                amount = full_math::mul_div(amount, U256::from(k.mantissa().unsigned_abs()), scale)
                    .ok_or_else(|| bad(ctx))?;
            }
        }
    }
    Ok(())
}
