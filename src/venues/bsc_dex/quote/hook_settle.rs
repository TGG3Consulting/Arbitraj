//! Файл: venues/bsc_dex/quote/hook_settle.rs
//! Что делает: решение «335 (лесенка влияния)» и решение 406 (t190, правило между точками) — точный выход (покупка
//! «с конца») с влиянием крючка B/C, зависящим от суммы (HookCurve::settle; вынесено из quote/hook_curve.rs — файл был
//! бы больше 200 строк). Вход пула зависит от доли крючка, а доля — от входа; нужно применённое влияние p, при котором
//! влияние на получившемся входе x(p) не больше p (расчёт не лучше лесенки):
//! 1. Подъём по upper (значение ближайшей большей точки, накопленный максимум): от наименьшего значения лесенки вверх
//!    «доля → вход → влияние»; значения — из конечного набора точек и растут строго, поэтому шагов не больше числа
//!    точек + 1. Итог подъёма p₀ годится и для правила котировки: at ≤ upper (quote/hook_rule.rs).
//! 2. Спуск по правилу котировки (at: max(хорда, продолжение наклона прошлого участка), не выше upper):
//!    p ← at(x(p)). Пока at(x) < p, следующий p меньше, вход x(p) не больше (вход растёт с
//!    влиянием), и at(x(p)) ≤ p (at неубывает по входу — накопленный максимум) — каждый шаг спуска снова годится,
//!    расчёт приближается к неподвижной точке сверху. Шагов — не больше «число точек + 2» (из данных); остановка
//!    раньше — годный итог, чуть выше точного. Шаг, у которого расчёт отказал или «at ≤ p» не выполнилось, не
//!    берётся.
//! 3. Лимит hooks.max_impact_* — к влиянию на итоговом входе (HookCurve::pct_at — правило котировки с лимитом).
//!
//! Кто вызывает: quote/direct.rs (calc_u, покупка), quote/route_calc.rs (buy_step); тесты на записях узла.
//! От чего зависит: quote/hook_curve.rs (HookCurve::value, pct_at).
//! Настройки: нет своих (лимит передаёт quote/select.rs).

use alloy_primitives::U256;
use rust_decimal::Decimal;

use super::hook_curve::HookCurve;

impl HookCurve {
    /// Что делает: точный выход (см. шапку). Принимает: влияние одним числом, расчёт `f` (влияние → (вход пула,
    /// итог)), перевод текста причины в ошибку вызывающего. Возвращает: применённое влияние и итог расчёта с ним.
    /// Ошибки: как у `f` (на подъёме); причины «не измерено», «больше лимита», «итерация не сошлась» — через `err`.
    pub fn settle<T, E>(
        &self,
        flat: Decimal,
        mut f: impl FnMut(Decimal) -> Result<(U256, T), E>,
        err: impl Fn(String) -> E,
    ) -> Result<(Decimal, T), E> {
        let rounds = self.len() + 2;
        let mut pct = self.floor(flat);
        let mut got: Option<(U256, T)> = None;
        for _ in 0..rounds {
            let (x, t) = f(pct)?;
            let need = self.value(flat, x, true).map_err(&err)?;
            if need <= pct {
                got = Some((x, t));
                break;
            }
            pct = need;
        }
        let Some((mut x, mut t)) = got else {
            return Err(err("влияние крючка по сумме не сошлось".into()));
        };
        for _ in 0..rounds {
            let Ok(v) = self.value(flat, x, false) else {
                break;
            };
            if v >= pct {
                break;
            }
            let Ok((x2, t2)) = f(v) else {
                break;
            };
            match self.value(flat, x2, false) {
                Ok(need) if need <= v => (pct, x, t) = (v, x2, t2),
                _ => break,
            }
        }
        self.pct_at(flat, x).map_err(&err)?;
        Ok((pct, t))
    }

    /// Наименьшее влияние лесенки (без лесенки — одно число), не меньше 0. Ошибок нет.
    fn floor(&self, flat: Decimal) -> Decimal {
        self.steps
            .as_deref()
            .and_then(|s| s.iter().map(|p| p.pct.max(Decimal::ZERO)).min())
            .unwrap_or(flat)
            .max(Decimal::ZERO)
    }
}
