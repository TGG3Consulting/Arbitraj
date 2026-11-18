//! Файл: venues/bsc_dex/quote/hook_curve.rs
//! Что делает: решение «335 (лесенка влияния)» — влияние крючка пула B/C как функция суммы входа в пул и стороны.
//! Лесенка стороны (HookImpact.buy / .sell — точки «вход в пул → влияние, %», по возрастанию входа) меряется циклом
//! крючков на нескольких суммах на одном блоке (check/hooks_head.rs) и пробой перед сделкой на сумме сделки
//! (exec/trade_probe.rs). Котировка берёт влияние на своём входе (решение 406, t190, Тигран 18.11.2026):
//! - значение точки — накопленный максимум по ней и всем меньшим суммам, у ЛЮБОЙ лесенки (и «с убыванием»: прежнее
//!   «наибольшее по всем точкам» и «ниже наименьшей — не измерено» убраны; запись 18.11 BTCMT: 500 → 4,696 %,
//!   850 → 4,584 % — на 45 USDT было 4,696 % при факте 1,72 %). x не больше наименьшей точки — её значение; x выше
//!   наибольшей — «не измерено на такой сумме» (пул на этой сумме не идёт); open (ревью Б1: путь сделки до пробы,
//!   продажа выхода, цикл DEX ↔ DEX) — вне измеренного наибольшее значение лесенки;
//! - upper — значение ближайшей большей точки (правило до решения 406); at — линейная интерполяция по входу между
//!   соседними точками; with_margin — at × (1 + hooks.impact_interp_margin_pct/100), но не выше upper (в точке — её
//!   значение без запаса: факт пробы на точной сумме не завышается). Котировка (value) — with_margin, если запас
//!   задан (HookCurve.margin), иначе upper. Отрицательное влияние — 0 (quote/quoter.rs less_hook, решение 261). Лимит
//!   hooks.max_impact_b_pct / _c_pct — к влиянию на этой сумме.
//!   Факты: запись 18.11 (tests/bsc_dex_hook_curve_t190.rs) — между точками факт и выше, и ниже интерполяции.
//! - settle (quote/hook_settle.rs): точный выход (покупка «с конца») — подъём по upper, затем спуск по value.
//! - cut (ревью Б2): проба перед сделкой на сумме x не прошла — точки стороны с входом ≥ x убираются (суммы от x —
//!   «не измерено»), меньшие суммы и другая сторона работают (check/hook_fold.rs);
//! - merge: точка пробы перед сделкой — в лесенку стороны (заменяет точку того же входа или вставляется); у
//!   неубывающей лесенки и точки не ниже наименьшей суммы — выравнивание накопленным максимумом (ревью 4 Н2), иначе
//!   вставка как есть (сырые факты) — накопленный максимум всё равно берёт pick.
//!
//! Кто вызывает: quote/select.rs (Usable.hook), quote/direct.rs, quote/route_calc.rs, quote/graph_seq.rs,
//! quote/hook_settle.rs (value), check/hook_fold.rs (merge), check/reprobe_rule.rs (merge); тесты на записях узла.
//! От чего зависит: core/types (HookStep).
//! Настройки: нет своих (лимит и запас hooks.impact_interp_margin_pct передаёт quote/select.rs).

use std::sync::Arc;

use alloy_primitives::U256;
use rust_decimal::Decimal;

use crate::core::types::HookStep;

/// Влияние крючка стороны пула для котировки.
#[derive(Debug, Clone, Default)]
pub struct HookCurve {
    /// Лесенка стороны; None или пусто — влияние одно на все суммы (Usable.hook_pct).
    pub steps: Option<Arc<Vec<HookStep>>>,
    /// Лимит влияния на сумме сделки: (имя настройки, %); None — не применяется (крючка B/C нет, продажа выхода).
    pub limit: Option<(&'static str, Decimal)>,
    /// Оценка (ревью Б1, см. шапку): сумма вне измеренного — наибольшее значение лесенки, а не «не измерено».
    pub open: bool,
    /// Запас интерполяции, % (hooks.impact_interp_margin_pct, решение 406); None — не задан: правило upper.
    pub margin: Option<Decimal>,
}

/// Настройка: запас интерполяции влияния крючка, % от значения (решение 406).
pub const MARGIN: &str = "hooks.impact_interp_margin_pct";

/// Разрешение влияния точки, % (ревью 3 Н-д): выход квотера известен с точностью до одной минимальной единицы, значит
/// влияние — до 100 / выход; выход не сохранён — 0. Ошибок нет.
pub fn resolution(s: &HookStep) -> Decimal {
    s.quoter_out
        .and_then(|o| super::super::num::to_decimal(o, 0))
        .filter(|o| *o > Decimal::ZERO)
        .and_then(|o| Decimal::ONE_HUNDRED.checked_div(o))
        .unwrap_or(Decimal::ZERO)
}

/// Неубывающая ли лесенка (по влиянию, отрицательное — 0). Ревью 3 Н-д: падение не больше разрешения двух соседних
/// точек (resolution) — шум измерения, не убывание (для merge; pick берёт накопленный максимум у любой). Ошибок нет.
pub fn monotone(steps: &[HookStep]) -> bool {
    steps.windows(2).all(|w| {
        let tol = resolution(&w[0]) + resolution(&w[1]);
        w[0].pct.max(Decimal::ZERO) <= w[1].pct.max(Decimal::ZERO) + tol
    })
}

/// Что делает: интерполяция на входе `x` (см. шапку). Принимает: лесенку (по возрастанию входа), вход. Возвращает:
/// влияние, % (не меньше 0); None — пусто или x больше наибольшего входа. Ошибок нет.
pub fn at(steps: &[HookStep], x: U256) -> Option<Decimal> {
    pick(steps, x, false)
}

/// Что делает: значение ближайшей большей точки (накопленный максимум). Принимает и возвращает — как at. Ошибок нет.
pub fn upper(steps: &[HookStep], x: U256) -> Option<Decimal> {
    pick(steps, x, true)
}

/// Что делает: интерполяция с запасом `margin` %, не выше upper (см. шапку). Принимает: лесенку, вход, запас.
/// Возвращает: влияние, %; None — как at. Ошибок нет.
pub fn with_margin(steps: &[HookStep], x: U256, margin: Decimal) -> Option<Decimal> {
    let hi = upper(steps, x)?;
    let k = Decimal::ONE + margin / Decimal::ONE_HUNDRED;
    Some(at(steps, x)?.checked_mul(k).map_or(hi, |v| v.min(hi)))
}

/// Общий разбор at / upper: `up` — значение ближайшей большей точки, иначе интерполяция. Ошибок нет.
fn pick(steps: &[HookStep], x: U256, up: bool) -> Option<Decimal> {
    if x > steps.last()?.amount_in {
        return None;
    }
    let j = steps.iter().position(|s| s.amount_in >= x)?;
    let hi = steps[..=j].iter().map(|s| s.pct).max()?;
    let v = match j.checked_sub(1) {
        Some(i) if !up && steps[j].amount_in > x => {
            let lo = steps[..=i].iter().map(|s| s.pct).max()?;
            lerp((steps[i].amount_in, lo), (steps[j].amount_in, hi), x)
        }
        _ => hi,
    };
    Some(v.max(Decimal::ZERO))
}

/// Линейная интерполяция по входу между точками (a0, v0) и (a1, v1), a0 < x < a1, v0 ≤ v1 (решение 406). Отрицательные
/// значения — 0. Вход не переводится в Decimal (больше 96 бит) — v1 (прежнее правило: не лучше ближайшей большей).
/// Ошибок нет.
fn lerp((a0, v0): (U256, Decimal), (a1, v1): (U256, Decimal), x: U256) -> Decimal {
    let (v0, v1) = (v0.max(Decimal::ZERO), v1.max(Decimal::ZERO));
    share(x - a0, a1 - a0)
        .and_then(|w| (v1 - v0).checked_mul(w))
        .map_or(v1, |p| (v0 + p).max(v0).min(v1))
}

/// Доля part / whole в Decimal; None — число не переводится в Decimal или whole = 0. Ошибок нет.
fn share(part: U256, whole: U256) -> Option<Decimal> {
    let dec = |u: U256| super::super::num::to_decimal(u, 0);
    dec(part)?.checked_div(dec(whole)?)
}

/// Наибольшее влияние лесенки, не меньше 0; None — пусто. Ошибок нет.
pub fn top_pct(steps: &[HookStep]) -> Option<Decimal> {
    steps.iter().map(|s| s.pct.max(Decimal::ZERO)).max()
}

/// Что делает: ревью Б2 — точки с входом ≥ x убираются (см. шапку). Принимает: лесенку, вход. Возвращает: оставшиеся
/// точки. Ошибок нет.
pub fn cut(steps: &[HookStep], x: U256) -> Vec<HookStep> {
    steps.iter().filter(|s| s.amount_in < x).cloned().collect()
}

/// Что делает: точка пробы перед сделкой в лесенку стороны (см. шапку). Принимает: лесенку, точку. Возвращает: новую
/// лесенку (по возрастанию входа; выравнивание — только у неубывающей лесенки и точки не ниже наименьшей суммы, ревью 4
/// Н2). Ошибок нет.
pub fn merge(steps: &[HookStep], p: HookStep) -> Vec<HookStep> {
    let level = steps.first().is_none_or(|low| p.amount_in >= low.amount_in) && monotone(steps);
    let mut out: Vec<HookStep> = steps
        .iter()
        .filter(|s| s.amount_in != p.amount_in)
        .cloned()
        .collect();
    let at = out.partition_point(|s| s.amount_in < p.amount_in);
    out.insert(at, p);
    if !level {
        return out;
    }
    let mut run = Decimal::MIN;
    for s in &mut out {
        run = run.max(s.pct);
        s.pct = run;
    }
    out
}

impl HookCurve {
    /// Что делает: влияние на входе `x` с лимитом (см. шапку). Принимает: влияние одним числом (без лесенки), вход.
    /// Возвращает: влияние, %. Ошибки (текст причины): не измерено на такой сумме; больше лимита.
    pub fn pct_at(&self, flat: Decimal, x: U256) -> Result<Decimal, String> {
        let v = self.value(flat, x, false)?;
        match self.limit {
            Some((name, limit)) if v > limit => Err(format!(
                "влияние крючка на этой сумме {v}% больше {name} ({limit}%)"
            )),
            _ => Ok(v),
        }
    }

    /// Что делает: влияние на входе `x` без лимита: `up` — upper, иначе правило котировки (with_margin при заданном
    /// запасе, иначе upper). Принимает: одно число (без лесенки), вход, правило. Возвращает: влияние, %. Ошибки: «не
    /// измерено на такой сумме».
    pub(super) fn value(&self, flat: Decimal, x: U256, up: bool) -> Result<Decimal, String> {
        let Some(s) = self.steps.as_deref().filter(|s| !s.is_empty()) else {
            return Ok(flat);
        };
        let v = match self.margin {
            Some(m) if !up => with_margin(s, x, m),
            _ => upper(s, x),
        };
        match (v, self.open) {
            (Some(v), _) => Ok(v),
            (None, true) => Ok(top_pct(s).unwrap_or(flat)),
            (None, false) => Err(format!(
                "влияние крючка на такой сумме не измерено (вход {x} вне измеренных точек лесенки)"
            )),
        }
    }

    /// Число точек лесенки (0 — без лесенки). Ошибок нет.
    pub fn len(&self) -> usize {
        self.steps.as_ref().map_or(0, |s| s.len())
    }

    /// Пуста ли лесенка. Ошибок нет.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
