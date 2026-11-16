//! Файл: core/bound_best.rs
//! Что делает: решение 372 — максимумы доказанной границы прибыли U(q) (bound.rs) на отрезке [0, q_hi] и проверка
//! порогов. U — кусочно-линейная по q: изломы — уровни asks, прообразы уровней bids через arr⁺ и излом arr⁺; максимум на
//! отрезке — в одном из изломов или концов (проверяются все — вогнутость не нужна; объём 0 не торгуется — на отрезке
//! (0, q₁] U линейна, поэтому нужно U(q₁) < порога и U(0) ≤ порога, Best::below_usd). То же для V(q) = U(q) −
//! порог%/100·C_A(q) (проверка порога %). Выделено из bound.rs (ревью t162, Н10: файл ≤ 200 строк).
//! Кто вызывает: core/sieve.rs (отсев), тесты.
//! От чего зависит: bound.rs (Bound, Buy, Sell, q_for, u_at), core/types (PriceLevel).
//! Настройки: нет.

use rust_decimal::Decimal;

use crate::core::types::PriceLevel;

use super::bound::{Bound, Buy, Sell};

/// Максимумы границы.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Best {
    /// max U по изломам q > 0 и объём, где он достигнут.
    pub max_u: Decimal,
    pub q_u: Decimal,
    /// U в точке q = 0 (объём 0 не торгуется, но предел U при q → 0+).
    pub u0: Decimal,
    /// max V по изломам q > 0 (проверка порога %; None — порог % отрицательный, проверки нет).
    pub max_v: Option<Decimal>,
    /// V в точке q = 0.
    pub v0: Option<Decimal>,
    /// Верх объёма покупки, до которого граница смотрит.
    pub q_hi: Decimal,
    /// Сколько точек (изломов) проверено.
    pub points: usize,
}

/// Накопленные количества уровней. None — переполнение.
fn cum(levels: &[PriceLevel]) -> Option<Vec<Decimal>> {
    let mut acc = Decimal::ZERO;
    let mut out = Vec::with_capacity(levels.len());
    for l in levels {
        acc = acc.checked_add(l.qty)?;
        out.push(acc);
    }
    Some(out)
}

impl Bound {
    /// Что делает: изломы U на [0, q_hi] и сам q_hi (см. шапку). Возвращает: (точки, q_hi); None — обе стороны DEX,
    /// переполнение, нельзя обратить приход.
    fn kinks(&self) -> Option<(Vec<Decimal>, Decimal)> {
        let mut points = vec![Decimal::ZERO];
        let mut q_hi = None;
        if let Buy::Book { levels, .. } = &self.buy {
            let c = cum(levels)?;
            q_hi = c.last().copied();
            points.extend(c);
        }
        if let Sell::Book { levels, .. } = &self.sell {
            let c = cum(levels)?;
            for s in &c {
                points.push(self.q_for(*s)?);
            }
            // DEX на A: дальше глубины B выручка плоская, вложение растёт — максимум не дальше прообраза глубины B.
            if q_hi.is_none() {
                q_hi = Some(self.q_for(c.last().copied().unwrap_or_default())?);
            }
        }
        let q_hi = q_hi?;
        let a = &self.arrive;
        if a.pct > Decimal::ZERO && a.fix < Decimal::ZERO {
            // Излом arr⁺: x = x(1 − pct) − fix.
            let x = (-a.fix).checked_div(a.pct)?;
            if let Some(q) = self.q_for(x.checked_mul(a.keep)?) {
                points.push(q);
            }
        }
        points.push(q_hi);
        Some((points, q_hi))
    }

    /// Что делает: максимумы U и V на [0, q_hi] по всем изломам (см. шапку). Принимает: порог % (None или < 0 — без V).
    /// Возвращает: Best; None — границу посчитать нельзя (обе стороны DEX, переполнение, нельзя обратить приход).
    pub fn best(&self, min_pct: Option<Decimal>) -> Option<Best> {
        let (points, q_hi) = self.kinks()?;
        let pct = min_pct.filter(|p| !p.is_sign_negative());
        let v_of = |u: Decimal, c: Decimal| -> Option<Option<Decimal>> {
            match pct {
                Some(p) => Some(Some(
                    u.checked_sub(c.checked_mul(p)?.checked_div(Decimal::ONE_HUNDRED)?)?,
                )),
                None => Some(None),
            }
        };
        let (u0, c0) = self.u_at(Decimal::ZERO)?;
        let v0 = v_of(u0, c0)?;
        let (mut max_u, mut q_u, mut max_v, mut n) =
            (None::<Decimal>, Decimal::ZERO, None::<Decimal>, 1);
        for q in points
            .into_iter()
            .filter(|q| *q > Decimal::ZERO && *q <= q_hi)
        {
            let (u, c) = self.u_at(q)?;
            n += 1;
            if max_u.is_none_or(|m| u > m) {
                (max_u, q_u) = (Some(u), q);
            }
            if let Some(v) = v_of(u, c)? {
                max_v = Some(max_v.map_or(v, |m: Decimal| m.max(v)));
            }
        }
        Some(Best {
            max_u: max_u?,
            q_u,
            u0,
            max_v,
            v0,
            q_hi,
            points: n,
        })
    }
}

impl Best {
    /// Что делает: доказано ли, что прибыль ниже порога в USDT на ЛЮБОМ объёме q > 0: max U по изломам q > 0 < порога и
    /// U(0) ≤ порога (на первом отрезке (0, q₁] U линейна между U(0) и U(q₁)). Принимает: порог. Ошибок нет.
    pub fn below_usd(&self, min_usd: Decimal) -> bool {
        self.max_u < min_usd && self.u0 <= min_usd
    }

    /// Что делает: доказано ли, что прибыль ниже порога % на любом объёме q > 0 (то же для V = U − порог%·вложение
    /// против 0). false — проверки нет (порог % < 0). Ошибок нет.
    pub fn below_pct(&self) -> bool {
        matches!((self.max_v, self.v0), (Some(m), Some(z)) if m < Decimal::ZERO && z <= Decimal::ZERO)
    }
}
