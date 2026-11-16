//! Файл: core/bound.rs
//! Что делает: решение 361 — доказанная верхняя граница прибыли направления с переводом U(q) (проект
//! docs/notes/t162-proven-sieve-2026-11-17.md, раздел 1): для любого объёма покупки q прибыль полного расчёта
//! (costs.rs evaluate_capped) не больше U(q), а при пороге % — прибыль не больше U(q) и вложение не меньше C(q):
//!   U(q) = κ_B · R_B(arr⁺(q)) − C_A(q)·(1 + r_A) − F,
//! где C_A — вложение (стакан A — точная свёртка уровней тем же проходом, что gateway/frozen.rs walk; DEX A — q/ρ_покупки),
//! r_A — ставка комиссии покупки в котируемой монете, arr⁺ — верх прихода на B (bound_in.rs), R_B — выручка (стакан B —
//! свёртка bids, за глубиной — плоско, ниже нуля — по лучшей цене; DEX B — ρ_продажи·s), κ_B = max(0, 1 − k·σ·√T − r_B),
//! F — постоянные затраты (фикс возврата USDT). U — кусочно-линейная по q: изломы — уровни asks, прообразы уровней bids
//! через arr⁺ и излом arr⁺; максимум на отрезке [0, q_hi] — в одном из изломов или концов (проверяются все — вогнутость
//! не нужна; объём 0 не торгуется — на отрезке (0, q₁] U линейна, поэтому нужно U(q₁) < порога и U(0) ≤ порога,
//! Best::below_usd). То же для V(q) = U(q) − порог%/100·C_A(q) (проверка порога %). Функция переиспользуется подбором объёма.
//! Округление: к каждому значению прибавляется запас (|вложение| + |выручка|)·10^(−MAX_SCALE/2) — половина знаков Decimal
//! (ошибка округления Decimal полного расчёта и границы на порядки меньше).
//! Кто вызывает: core/bound_in.rs (сборка), core/sieve.rs (отсев), тесты.
//! От чего зависит: core/types (PriceLevel), rust_decimal.
//! Настройки: нет.

use rust_decimal::Decimal;

use crate::core::types::PriceLevel;

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
    /// Доля выручки, которая остаётся после запаса k·σ·√T (только базовая часть, решение 240) и комиссии продажи.
    pub kappa: Decimal,
    /// Постоянные затраты в USDT (не меньше 0 не требуется — берутся как есть).
    pub fixed: Decimal,
}

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
    fn q_for(&self, s: Decimal) -> Option<Decimal> {
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

    /// Что делает: максимумы U и V на [0, q_hi] по всем изломам (см. шапку). Принимает: порог % (None или < 0 — без V).
    /// Возвращает: Best; None — границу посчитать нельзя (обе стороны DEX, переполнение, нельзя обратить приход).
    pub fn best(&self, min_pct: Option<Decimal>) -> Option<Best> {
        let cum = |levels: &[PriceLevel]| -> Option<Vec<Decimal>> {
            let mut acc = Decimal::ZERO;
            let mut out = Vec::with_capacity(levels.len());
            for l in levels {
                acc = acc.checked_add(l.qty)?;
                out.push(acc);
            }
            Some(out)
        };
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
