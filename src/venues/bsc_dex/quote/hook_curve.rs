//! Файл: venues/bsc_dex/quote/hook_curve.rs
//! Что делает: решение «335 (лесенка влияния)» — влияние крючка пула B/C как функция суммы входа в пул и стороны.
//! Лесенка стороны (HookImpact.buy / .sell — точки «вход в пул → влияние, %», по возрастанию входа) меряется циклом
//! крючков на нескольких суммах на одном блоке (check/hooks_head.rs) и пробой перед сделкой на сумме сделки
//! (exec/trade_probe.rs). Котировка берёт влияние на своём входе:
//! - at (решение 405, t190): влияние на входе x неубывающей лесенки — ЛИНЕЙНАЯ ИНТЕРПОЛЯЦИЯ по входу между двумя
//!   соседними точками (меньшей и ближайшей большей); значение точки — накопленный максимум по точкам до неё
//!   включительно (шумовое падение в пределах разрешения не делает расчёт лучше факта, ревью 3 Н-д). x в точке — её
//!   значение; x не больше наименьшей точки — значение наименьшей (как до решения 405). Прежнее правило «значение
//!   ближайшей большей точки» (upper) занижало прибыль между точками и теряло сделки; оно осталось только верхней
//!   оценкой для подъёма в settle. Лесенка с убыванием (не монотонна) — наибольшее по всем точкам (верхняя оценка,
//!   без интерполяции, как до решения 405). x выше наибольшей измеренной суммы — «не измерено на такой сумме» (пул на
//!   этой сумме не идёт, как при неизмеренном влиянии); ревью Н2: у лесенки с убыванием и x НИЖЕ наименьшей точки —
//!   тоже «не измерено». Ревью Б1: open — оценка (путь сделки до пробы, продажа выхода, цикл DEX ↔ DEX): вне
//!   измеренного — наибольшее значение лесенки; итог сделки — на лесенке сделки после пробы на её сумме
//!   (exec/hook_trade.rs) или пробы eth_simulateV1 всего цикла (exec/cycle_trade.rs). Отрицательное влияние — 0 (как
//!   quote/quoter.rs less_hook, решение 261). Лимит hooks.max_impact_b_pct / _c_pct — к влиянию на этой сумме: больше
//!   — пул на этой сумме не идёт.
//!   Факт записей узла 16–17.11 — у крючка 0x32c59d… (GAIB, BTCMT) влияние покупки растёт с суммой (формулой
//!   check/hooks_probe.rs impact: 0 на 1 USDT, 0,0990 % / 0,6735 % у GAIB и 0,7443 % / 3,3658 % у BTCMT на 20 / 100
//!   USDT; к выходу квотера — 0,0991 % / 0,6781 % и 0,7499 % / 3,4830 %), у остальных — постоянное. Квотера на суммах
//!   между точками в записях нет: интерполяция — оценка между фактами (решение Тиграна 18.11.2026), не факт.
//! - settle (quote/hook_settle.rs): точный выход (покупка «с конца») — подъём по upper, затем спуск по интерполяции.
//! - cut (ревью Б2): проба перед сделкой на сумме x не прошла — точки стороны с входом ≥ x убираются (суммы от x —
//!   «не измерено»), меньшие суммы и другая сторона работают (check/hook_fold.rs);
//! - merge: точка пробы перед сделкой (на более новом блоке) — в лесенку стороны: заменяет точку того же входа или
//!   вставляется; затем лесенка выравнивается накопленным максимумом (значения только растут — верхняя оценка).
//!   Ревью 4 (Н2): выравнивание — только если прежняя лесенка неубывающая (monotone) и точка не ниже её наименьшей
//!   суммы. Лесенка с убыванием (цикл на одном блоке измерил падение влияния с суммой) или точка ниже наименьшей —
//!   вставка как есть (сырые факты): at сам видит убывание — ниже наименьшей точки «не измерено», внутри — наибольшее
//!   (выравнивание сделало бы лесенку неубывающей, и сумма ниже наименьшей точки снова считалась бы «измеренной»).
//!
//! Кто вызывает: quote/select.rs (Usable.hook), quote/direct.rs, quote/route_calc.rs, quote/graph_seq.rs,
//! quote/hook_settle.rs (value), check/hook_fold.rs (merge), check/reprobe_rule.rs (merge); тесты на записях узла.
//! От чего зависит: core/types (HookStep).
//! Настройки: нет своих (лимит передаёт quote/select.rs).

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
}

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
/// точек (resolution) — шум измерения, не убывание; значение at при этом — наибольшее по точкам до ближайшей большей
/// (шумовое падение не делает расчёт лучше факта). Ошибок нет.
pub fn monotone(steps: &[HookStep]) -> bool {
    steps.windows(2).all(|w| {
        let tol = resolution(&w[0]) + resolution(&w[1]);
        w[0].pct.max(Decimal::ZERO) <= w[1].pct.max(Decimal::ZERO) + tol
    })
}

/// Что делает: влияние на входе `x` (см. шапку, решение 405 — интерполяция). Принимает: лесенку (по возрастанию входа),
/// вход. Возвращает: влияние, % (не меньше 0); None — пусто, x больше наибольшего измеренного входа или (лесенка с
/// убыванием) меньше наименьшего. Ошибок нет.
pub fn at(steps: &[HookStep], x: U256) -> Option<Decimal> {
    pick(steps, x, false)
}

/// Что делает: прежнее правило (до решения 405) — значение ближайшей большей точки (накопленный максимум); верхняя
/// оценка для подъёма в settle (quote/hook_settle.rs). Принимает и возвращает — как at. Ошибок нет.
pub fn upper(steps: &[HookStep], x: U256) -> Option<Decimal> {
    pick(steps, x, true)
}

/// Общий разбор at / upper: `up` — значение ближайшей большей точки, иначе интерполяция. Ошибок нет.
fn pick(steps: &[HookStep], x: U256, up: bool) -> Option<Decimal> {
    let (low, top) = (steps.first()?, steps.last()?);
    if x > top.amount_in {
        return None;
    }
    let v = if monotone(steps) {
        let j = steps.iter().position(|s| s.amount_in >= x)?;
        let hi = steps[..=j].iter().map(|s| s.pct).max()?;
        match j.checked_sub(1) {
            Some(i) if !up && steps[j].amount_in > x => {
                let lo = steps[..=i].iter().map(|s| s.pct).max()?;
                lerp((steps[i].amount_in, lo), (steps[j].amount_in, hi), x)
            }
            _ => hi,
        }
    } else if x < low.amount_in {
        return None;
    } else {
        top_pct(steps)?
    };
    Some(v.max(Decimal::ZERO))
}

/// Линейная интерполяция по входу между точками (a0, v0) и (a1, v1), a0 < x < a1, v0 ≤ v1 (решение 405). Отрицательные
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
/// лесенку (по возрастанию входа; накопленный максимум — только у неубывающей лесенки и точки не ниже наименьшей
/// суммы, ревью 4 Н2). Ошибок нет.
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

    /// Что делает: влияние на входе `x` без лимита: `up` — прежнее правило (upper), иначе интерполяция (at). Принимает:
    /// одно число (без лесенки), вход, правило. Возвращает: влияние, %. Ошибки: «не измерено на такой сумме».
    pub(super) fn value(&self, flat: Decimal, x: U256, up: bool) -> Result<Decimal, String> {
        let Some(s) = self.steps.as_deref().filter(|s| !s.is_empty()) else {
            return Ok(flat);
        };
        match (pick(s, x, up), self.open) {
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
