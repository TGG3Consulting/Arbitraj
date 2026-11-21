//! Файл: venues/bsc_dex/exec/trade_hooks.rs
//! Что делает: решение «335 (лесенка влияния)», ревью 2–3 — факты пробы крючков B/C этой сделки (вынесено из
//! exec/hook_drive.rs ревью 4: файл был больше 200 строк; пути hook_drive::{Fresh, Want, TradeHooks} — реэкспорт):
//! - Fresh: точка пробы и влияние, которое расчёт применял на этой сумме до пробы;
//! - Want: вход шага с крючком в пути сделки, применённое влияние, лесенка, по которой он считался;
//! - TradeHooks: curve — лесенка пула для расчёта сделки (check/hook_fold.rs local); apply — допуск шага пути (лимит
//!   hooks.max_impact_* — только по факту пробы, ревью 3 Н-г); uncovered — входы, не покрытые фактом этой сделки;
//!   quoted — выход квотера пробы на входе (сверка прямого режима без второго вызова); подъём lift ставит Drive.
//!
//! Кто вызывает: exec/hook_drive.rs (Drive), exec/hook_trade.rs, exec/live.rs, exec/live_hops.rs (apply),
//! exec/trade_probe.rs (Fresh).
//! От чего зависит: check/hook_fold.rs (local, covered), quote/select.rs (Usable), core/types (HookStep, PoolRef, Side).
//! Настройки: нет.

use std::sync::Arc;

use alloy_primitives::U256;
use rust_decimal::Decimal;

use crate::core::types::{HookStep, PoolRef, Side};

use super::super::check::hook_fold;
use super::super::quote::select::Usable;

/// Точка пробы этой сделки: пул, сторона (покупка — вход встречный токен), точка лесенки с выходом квотера и влияние,
/// с которым расчёт считал этот вход до пробы.
#[derive(Debug, Clone, PartialEq)]
pub struct Fresh {
    pub pool: PoolRef,
    pub buy: bool,
    pub step: HookStep,
    pub est: Decimal,
}

/// Вход шага с крючком B/C в пути сделки, влияние, применённое расчётом на нём, и лесенка, по которой он считался
/// (входы точек — для «покрыт ли фактом сделки»).
#[derive(Debug, Clone, PartialEq)]
pub struct Want {
    pub pool: PoolRef,
    pub buy: bool,
    pub x: U256,
    pub applied: Decimal,
    pub steps: Vec<HookStep>,
}

/// Факты пробы этой сделки и подъём влияния для выбора суммы следующей пробы.
#[derive(Debug, Clone, Default)]
pub struct TradeHooks {
    pub fresh: Vec<Fresh>,
    pub(super) lift: Vec<(PoolRef, bool, Decimal)>,
}

impl TradeHooks {
    /// Точки этой сделки по пулу и стороне (по возрастанию входа). Ошибок нет.
    pub fn pts(&self, r: &PoolRef, buy: bool) -> Vec<HookStep> {
        let mut v: Vec<HookStep> = self
            .fresh
            .iter()
            .filter(|f| &f.pool == r && f.buy == buy)
            .map(|f| f.step.clone())
            .collect();
        v.sort_by_key(|s| s.amount_in);
        v
    }

    /// Что делает: лесенка пула для расчёта сделки (см. шапку). Принимает: пул, сторону, лесенку хранилища, прежнее
    /// одно число. Возвращает: (лесенка, одно число с подъёмом, применять ли лимит). Ошибок нет.
    pub fn curve(
        &self,
        r: &PoolRef,
        buy: bool,
        stored: &[HookStep],
        flat: Decimal,
    ) -> (Vec<HookStep>, Decimal, bool) {
        let fresh = self.pts(r, buy);
        let lift = self
            .lift
            .iter()
            .find(|l| &l.0 == r && l.1 == buy)
            .map(|l| l.2);
        let local = hook_fold::local(stored, flat, &fresh, lift);
        let flat = lift.map_or(flat, |l| flat.max(l));
        (local, flat, !fresh.is_empty() && lift.is_none())
    }

    /// Что делает: допуск пула для пути сделки — лесенка сделки, оценка вне измеренного (open), лимит — только по
    /// факту пробы. Пул без крючка B/C (`hooked` = false) — без изменений. Принимает: пул, сторону пула, крючок ли,
    /// допуск из quote/select.rs. Ошибок нет.
    pub fn apply(&self, r: &PoolRef, side: Side, hooked: bool, u: &mut Usable) {
        if !hooked {
            return;
        }
        let stored = u.hook.steps.clone().unwrap_or_default();
        let (local, flat, limit) = self.curve(r, side == Side::Buy, &stored, u.hook_pct);
        u.hook.steps = (!local.is_empty()).then(|| Arc::new(local));
        u.hook_pct = flat;
        u.hook.open = true;
        if !limit {
            u.hook.limit = None;
        }
    }

    /// Входы, не покрытые фактом этой сделки (без повторов). Ошибок нет.
    pub fn uncovered(&self, wants: &[Want]) -> Vec<Want> {
        let mut out: Vec<Want> = Vec::new();
        for w in wants {
            let fresh = self.pts(&w.pool, w.buy);
            let dup = out
                .iter()
                .any(|o| o.pool == w.pool && o.buy == w.buy && o.x == w.x);
            if !dup && !hook_fold::covered(&w.steps, &fresh, w.x) {
                out.push(w.clone());
            }
        }
        out
    }

    /// Выход квотера этой сделки на входе x пула и стороны (ревью 2 Н7). Ошибок нет.
    pub fn quoted(&self, r: &PoolRef, buy: bool, x: U256) -> Option<U256> {
        self.fresh
            .iter()
            .find(|f| &f.pool == r && f.buy == buy && f.step.amount_in == x)
            .and_then(|f| f.step.quoter_out)
    }

    /// Подъём для входа (см. шапку): p + max(0, p − e) по последней (наибольшей) точке сделки пула и стороны, не
    /// меньше применённого. Ошибок нет.
    pub(super) fn lift_of(&self, w: &Want) -> Decimal {
        let last = self
            .fresh
            .iter()
            .filter(|f| f.pool == w.pool && f.buy == w.buy)
            .max_by_key(|f| f.step.amount_in);
        let up = last.map_or(w.applied, |f| {
            f.step.pct + (f.step.pct - f.est).max(Decimal::ZERO)
        });
        up.max(w.applied)
    }
}
