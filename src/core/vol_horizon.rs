//! Файл: core/vol_horizon.rs
//! Что делает: решение 357 (только журнал, решение о сделке не меняет) — σ ряда цены на горизонте T: дисперсия
//! приращений лог-цены на непересекающихся отрезках длины T назад от «сейчас» (цена отрезка — последний замер не позже
//! его конца), σ_T = √(Σ Rⱼ² / (m · T)) за секунду, m — полные отрезки внутри покрытого рядом времени; среднее не
//! вычитается. Плюс автокорреляция соседних шагов ρ₁ = Σ rᵢ·rᵢ₋₁ / Σ rᵢ² (отскок цены — ρ₁ < 0).
//! Зачем: σ ряда vol.rs (Σr²/ΣΔt по каждому замеру) умножается на √T; при шуме цены (мерцание уровней, смена объёма
//! сравнения) шумовая часть входит в запас n·T/W раз, при тренде σ занижена. σ_T меряет ровно движение цены за T.
//! Доказательство и выбор оценки — docs/notes/t161-price-risk-2026-11-16.md (раздел В).
//! Кто вызывает: core/vol.rs (Volatility::horizon).
//! От чего зависит: rust_decimal, chrono, core/vol.rs (Step).
//! Настройки: нет (горизонт — T направления, окно — profit.sigma_window_min передаёт вызывающий).

use std::collections::VecDeque;

use chrono::{DateTime, Utc};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::{Decimal, MathematicalOps};
use serde::Serialize;

use super::vol::Step;

/// σ на горизонте T и признаки ряда за окно.
#[derive(Debug, Clone, Serialize)]
pub struct Horizon {
    /// σ на горизонте T за секунду (доля, как σ ряда); None — нет ни одного полного отрезка длины T.
    pub sigma_t: Option<Decimal>,
    /// Сколько полных отрезков длины T вошло (m).
    pub blocks: usize,
    /// Автокорреляция соседних шагов ряда; None — все шаги нулевые или шагов меньше двух.
    pub rho1: Option<Decimal>,
    /// Шагов ряда в окне и из них с ненулевым изменением цены.
    pub steps: usize,
    pub changes: usize,
}

/// Секунды от `t` до `now` (миллисекунды / 1000, как Δt в vol.rs). Ошибок нет.
fn secs(now: DateTime<Utc>, t: DateTime<Utc>) -> Decimal {
    Decimal::from((now - t).num_milliseconds()) / Decimal::ONE_THOUSAND
}

/// Что делает: σ на горизонте и ρ₁ по шагам окна (см. шапку). Шаг с временем позже now в отрезки не входит.
/// Принимает: шаги ряда по времени (старые первыми), «сейчас», горизонт T в секундах.
/// Возвращает: Horizon (σ_T None — T ≤ 0 или ряд короче T). Ошибок нет.
pub fn of(steps: &VecDeque<Step>, now: DateTime<Utc>, h: Decimal) -> Horizon {
    let changes = steps.iter().filter(|(_, r, _)| !r.is_zero()).count();
    let (mut sq, mut cross) = (Decimal::ZERO, Decimal::ZERO);
    let mut prev: Option<Decimal> = None;
    for (_, r, _) in steps {
        sq += r * r;
        if let Some(p) = prev {
            cross += p * *r;
        }
        prev = Some(*r);
    }
    let rho1 = if steps.len() > 1 && !sq.is_zero() {
        Some(cross / sq)
    } else {
        None
    };
    let mut out = Horizon {
        sigma_t: None,
        blocks: 0,
        rho1,
        steps: steps.len(),
        changes,
    };
    // Покрытое рядом время: от замера перед первым шагом (его Δt) до now.
    let Some((t0, _, dt0)) = steps.front() else {
        return out;
    };
    if h <= Decimal::ZERO {
        return out;
    }
    let m = ((secs(now, *t0) + dt0) / h).floor().to_usize().unwrap_or(0);
    if m == 0 {
        return out;
    }
    // Rⱼ — сумма лог-доходностей шагов отрезка j (0 — самый поздний: (now − T; now]).
    let mut sums = vec![Decimal::ZERO; m];
    for (t, r, _) in steps {
        let x = secs(now, *t);
        if x < Decimal::ZERO {
            continue;
        }
        if let Some(j) = (x / h).floor().to_usize().filter(|j| *j < m) {
            sums[j] += *r;
        }
    }
    let total: Decimal = sums.iter().map(|r| r * r).sum();
    out.blocks = m;
    out.sigma_t = (total / (Decimal::from(m) * h)).sqrt();
    out
}
