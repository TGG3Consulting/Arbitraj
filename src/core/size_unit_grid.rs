//! Сетка объёмов для DEX → биржа с разными единицами. Номер шага задаёт количество на B;
//! покупка округляется вверх до первой физически достаточной единицы токена на A.

use rust_decimal::{Decimal, RoundingStrategy, prelude::ToPrimitive};

use super::{dirctx::DirCtx, settlement_unit::UnitRatio};

#[derive(Clone, Copy, Debug)]
pub struct SellGrid {
    units: UnitRatio,
    sell_step: Decimal,
    raw_atom: Decimal,
}

impl SellGrid {
    /// A — кошелёк с raw-токеном. Если шаг B меньше одного атома, достаточно целых атомов A.
    pub(super) fn for_dex_buy(
        units: UnitRatio,
        sell_step: Decimal,
    ) -> Result<(Decimal, Option<Self>), String> {
        let step = units.to_buy(sell_step)?;
        if units.is_identity() {
            return Ok((step, None));
        }
        let decimals = units
            .raw_decimals
            .ok_or("неизвестны decimals шага покупки DEX")?;
        let atom = Decimal::from_i128_with_scale(1, u32::from(decimals).min(Decimal::MAX_SCALE));
        if step <= atom {
            return Ok((atom, None));
        }
        if step.scale() <= atom.scale() && units.to_sell(step)? == sell_step {
            return Ok((step, None));
        }
        Ok((
            step,
            Some(Self {
                units,
                sell_step,
                raw_atom: atom,
            }),
        ))
    }

    pub(super) fn qty(self, n: u64) -> Option<Decimal> {
        let target = Decimal::from(n).checked_mul(self.sell_step)?;
        let qty = self.units.to_buy(target).ok()?;
        let floor = qty.round_dp_with_strategy(self.raw_atom.scale(), RoundingStrategy::ToZero);
        if self.units.to_sell(floor).ok()? >= target {
            return Some(floor);
        }
        // У больших объёмов Decimal может представлять меньше десятичных знаков, чем ERC-20.
        let mut increment = self.raw_atom;
        loop {
            let up = floor.checked_add(increment)?;
            if up > floor && self.units.to_sell(up).ok()? >= target {
                return Some(up);
            }
            increment = increment.checked_mul(Decimal::TEN)?;
        }
    }

    pub(super) fn index(self, qty: Decimal) -> Option<u64> {
        let in_b = self.units.to_sell(qty).ok()?;
        let whole = in_b.checked_sub(in_b.checked_rem(self.sell_step)?)?;
        let n = whole.checked_div(self.sell_step)?.to_u64()?;
        if self.qty(n)? <= qty {
            Some(n)
        } else {
            n.checked_sub(1)
        }
    }
}

/// Тот же шаг для быстрого финального пересчёта, когда первый подбор уже выбрал объём A.
pub(super) fn floor_buy(cx: &DirCtx, qty: Decimal, step: Decimal) -> Result<Decimal, String> {
    if cx.pair_a.is_none()
        && !cx.units_b_per_a.is_identity()
        && let Some(b) = &cx.pair_b
        && let (_, Some(grid)) = SellGrid::for_dex_buy(cx.units_b_per_a, b.step_size)?
    {
        return grid
            .index(qty)
            .and_then(|n| grid.qty(n))
            .ok_or_else(|| "объём покупки не помещается в сетку продажи".into());
    }
    Ok((qty / step).floor() * step)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn a_non_terminating_inverse_uses_the_smallest_sufficient_raw_amount() {
        // Та же шкала 18 знаков, что у записанного контракта SNDKB; коэффициент 3 — проверка арифметики.
        let ratio = UnitRatio {
            sell_per_raw: Decimal::from(3),
            raw_decimals: Some(18),
            ..UnitRatio::IDENTITY
        };
        let (_, grid) = SellGrid::for_dex_buy(ratio, Decimal::ONE).unwrap();
        let grid = grid.unwrap();
        let atom = Decimal::from_i128_with_scale(1, 18);
        for (n, want) in [
            (1, "0.333333333333333334"),
            (2, "0.666666666666666667"),
            (3, "1"),
        ] {
            let amount = Decimal::from_str(want).unwrap();
            assert_eq!(grid.qty(n), Some(amount));
            assert_eq!(grid.index(amount), Some(n));
            assert_eq!(grid.index(amount - atom), Some(n - 1));
        }
    }
}
