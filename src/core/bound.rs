//! Файл: core/bound.rs
//! Что делает: решение 372 — доказанная верхняя граница прибыли направления с переводом U(q) (проект
//! docs/notes/t162-proven-sieve-2026-11-17.md, раздел 1): для любого объёма покупки q прибыль полного расчёта
//! (costs.rs evaluate_capped) не больше U(q), а при пороге % — прибыль не больше U(q) и вложение не меньше C(q):
//!   U(q) = κ_B · R_B(arr⁺(q)) − C_A(q)·(1 + r_A) − F,
//! где C_A — вложение (стакан A — точная свёртка уровней тем же проходом, что gateway/frozen.rs walk; DEX A — q/ρ_покупки),
//! r_A — ставка комиссии покупки в котируемой монете, arr⁺ — верх прихода на B (bound_in.rs), R_B — выручка (стакан B —
//! свёртка bids, за глубиной — плоско, ниже нуля — по лучшей цене; DEX B — ρ_продажи·s),
//! κ_B = max(0, e^(−k·σ·√T) − r_B) (решение 390, t176; прежде 1 − k·σ·√T − r_B под линейный запас),
//! F — постоянные затраты (фикс возврата USDT). Максимумы по изломам (проверка порогов) — bound_best.rs.
//! Округление: к каждому значению прибавляется запас (|вложение| + |выручка|)·10^(−MAX_SCALE/2) — половина знаков Decimal
//! (ошибка округления Decimal полного расчёта и границы на порядки меньше).
//! Кто вызывает: core/bound_in.rs (сборка), core/bound_best.rs (максимумы), core/sieve.rs (отсев), тесты.
//! От чего зависит: core/types (PriceLevel), rust_decimal.
//! Настройки: нет.

use rust_decimal::Decimal;

use crate::core::types::PriceLevel;

pub use super::bound_best::Best;

/// Сторона покупки на A.
#[derive(Debug, Clone)]
pub enum Buy {
    /// Стакан asks (порядок прохода — как у площадки), ставки комиссии: в котируемой (от суммы) и в монете (от количества).
    Book {
        levels: Vec<PriceLevel>,
        fee_quote: Decimal,
        fee_coin: Decimal,
    },
    /// DEX: верх монет за 1 USDT (вложение ≥ q/ρ).
    Dex { rho: Decimal },
}

/// Сторона продажи на B.
#[derive(Debug, Clone)]
pub enum Sell {
    /// Стакан bids и ставка комиссии продажи в котируемой монете (от суммы).
    Book {
        levels: Vec<PriceLevel>,
        fee_quote: Decimal,
    },
    /// DEX: верх USDT за 1 монету.
    Dex { rho: Decimal },
}

/// Верх прихода: arr⁺(x) = min(x, x·(1 − pct) − fix)·keep, x — выводимое количество (bound_in.rs).
#[derive(Debug, Clone)]
pub struct Arrive {
    pub pct: Decimal,
    pub fix: Decimal,
    pub keep: Decimal,
}

/// Граница направления (все входы — числа ядра, без котировок).
#[derive(Debug, Clone)]
pub struct Bound {
    pub buy: Buy,
    pub sell: Sell,
    pub arrive: Arrive,
    /// Доля выручки, которая остаётся после запаса (только базовая часть 1 − e^(−k·σ·√T), решения 240 и 390) и комиссии
    /// продажи.
    pub kappa: Decimal,
    /// Постоянные затраты в USDT (не меньше 0 не требуется — берутся как есть).
    pub fixed: Decimal,
}

/// Проход по уровням (тот же, что gateway/frozen.rs walk): (набрано, сумма). Ошибок нет; None — переполнение.
fn walk(levels: &[PriceLevel], qty: Decimal) -> Option<(Decimal, Decimal)> {
    let (mut left, mut filled, mut notional) = (qty, Decimal::ZERO, Decimal::ZERO);
    for l in levels {
        if left <= Decimal::ZERO {
            break;
        }
        let take = left.min(l.qty);
        filled = filled.checked_add(take)?;
        notional = notional.checked_add(take.checked_mul(l.price)?)?;
        left -= take;
    }
    Some((filled, notional))
}

impl Bound {
    /// Выводимое количество x(q): минус комиссия покупки в монете. Ошибок нет; None — переполнение.
    fn x_of(&self, q: Decimal) -> Option<Decimal> {
        match &self.buy {
            Buy::Book { fee_coin, .. } => q.checked_mul(Decimal::ONE - fee_coin),
            Buy::Dex { .. } => Some(q),
        }
    }

    /// Верх прихода на B. None — переполнение.
    pub fn arr(&self, q: Decimal) -> Option<Decimal> {
        let a = &self.arrive;
        let x = self.x_of(q)?;
        let net = x.checked_mul(Decimal::ONE - a.pct)?.checked_sub(a.fix)?;
        x.min(net).checked_mul(a.keep)
    }

    /// Нижняя граница вложения C_A(q) (без комиссии) и комиссия покупки в котируемой. None — переполнение.
    fn cost(&self, q: Decimal) -> Option<(Decimal, Decimal)> {
        match &self.buy {
            Buy::Book {
                levels, fee_quote, ..
            } => {
                let (_, n) = walk(levels, q)?;
                Some((n, n.checked_mul(*fee_quote)?))
            }
            Buy::Dex { rho } => Some((q.checked_div(*rho)?, Decimal::ZERO)),
        }
    }

    /// Верх выручки R_B(s) до запаса. None — переполнение.
    fn revenue(&self, s: Decimal) -> Option<Decimal> {
        match &self.sell {
            Sell::Book { levels, .. } => {
                if s < Decimal::ZERO {
                    return s.checked_mul(levels.first()?.price);
                }
                Some(walk(levels, s)?.1)
            }
            Sell::Dex { rho } => s.checked_mul(*rho),
        }
    }

    /// Что делает: U(q) с запасом округления и нижняя граница вложения. Принимает: объём покупки. Возвращает:
    /// (U, вложение); None — переполнение (вызывающий не отсекает).
    pub fn u_at(&self, q: Decimal) -> Option<(Decimal, Decimal)> {
        let (c, fee) = self.cost(q)?;
        let r = self.revenue(self.arr(q)?)?;
        let u = self
            .kappa
            .checked_mul(r)?
            .checked_sub(c)?
            .checked_sub(fee)?
            .checked_sub(self.fixed)?;
        let guard = c
            .abs()
            .checked_add(r.abs())?
            .checked_mul(Decimal::new(1, Decimal::MAX_SCALE / 2))?;
        Some((u.checked_add(guard)?, c))
    }

    /// Объём покупки, при котором приход arr⁺ достигает s (наименьший). None — нельзя обратить (keep ≤ 0, pct ≥ 1).
    pub(super) fn q_for(&self, s: Decimal) -> Option<Decimal> {
        let a = &self.arrive;
        if a.keep <= Decimal::ZERO || a.pct >= Decimal::ONE {
            return None;
        }
        let base = s.checked_div(a.keep)?;
        let x = base.max(base.checked_add(a.fix)?.checked_div(Decimal::ONE - a.pct)?);
        match &self.buy {
            Buy::Book { fee_coin, .. } if *fee_coin < Decimal::ONE => {
                x.checked_div(Decimal::ONE - fee_coin)
            }
            Buy::Book { .. } => None,
            Buy::Dex { .. } => Some(x),
        }
    }
}
