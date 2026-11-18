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
//! - upper — значение ближайшей большей точки; chord — хорда (линейная интерполяция по входу) между соседними
//!   точками a1 < x < a2; at — правило котировки (решение 406, Тигран 18.11): max(хорда в x, продолжение наклона
//!   предыдущего участка [a0, a1] от a1 до x), но не выше значения a2 (upper); на первом участке (нет a0) — хорда.
//!   В точке — её значение. Для ускоряющегося роста хорда выше факта, для замедляющегося — продолжение наклона.
//!   Отрицательное влияние — 0 (quote/quoter.rs less_hook, решение 261). Лимит hooks.max_impact_b_pct / _c_pct — к
//!   влиянию на этой сумме. Факты — запись 18.11 (tests/bsc_dex_hook_curve_t190.rs).
//! - settle (quote/hook_settle.rs): точный выход (покупка «с конца») — подъём по upper, затем спуск по at.
//! - cut (ревью Б2): проба перед сделкой на сумме x не прошла — точки стороны с входом ≥ x убираются (суммы от x —
//!   «не измерено»), меньшие суммы и другая сторона работают (check/hook_fold.rs);
//! - merge: точка пробы перед сделкой — в лесенку стороны (заменяет точку того же входа или вставляется); у
//!   неубывающей лесенки и точки не ниже наименьшей суммы — выравнивание накопленным максимумом (ревью 4 Н2), иначе
//!   вставка как есть (сырые факты) — накопленный максимум всё равно берёт pick.
//!
//! Кто вызывает: quote/select.rs (Usable.hook), quote/direct.rs, quote/route_calc.rs, quote/graph_seq.rs,
//! quote/hook_settle.rs (value), check/hook_fold.rs (merge), check/reprobe_rule.rs (merge); тесты на записях узла.
//! От чего зависит: core/types (HookStep), quote/hook_rule.rs (at, upper, chord — реэкспорт).
//! Настройки: нет своих (лимит передаёт quote/select.rs).

use std::sync::Arc;

use alloy_primitives::U256;
use rust_decimal::Decimal;

use crate::core::types::HookStep;

/// Правила значения между точками — quote/hook_rule.rs (вынесено: файл был бы больше 200 строк).
pub use super::hook_rule::{at, chord, upper};

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
/// точек (resolution) — шум измерения, не убывание (для merge; pick берёт накопленный максимум у любой). Ошибок нет.
pub fn monotone(steps: &[HookStep]) -> bool {
    steps.windows(2).all(|w| {
        let tol = resolution(&w[0]) + resolution(&w[1]);
        w[0].pct.max(Decimal::ZERO) <= w[1].pct.max(Decimal::ZERO) + tol
    })
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

    /// Что делает: влияние на входе `x` без лимита: `up` — upper, иначе правило котировки at. Принимает: одно число
    /// (без лесенки), вход, правило. Возвращает: влияние, %. Ошибки: «не измерено на такой сумме».
    pub(super) fn value(&self, flat: Decimal, x: U256, up: bool) -> Result<Decimal, String> {
        let Some(s) = self.steps.as_deref().filter(|s| !s.is_empty()) else {
            return Ok(flat);
        };
        let v = if up { upper(s, x) } else { at(s, x) };
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
