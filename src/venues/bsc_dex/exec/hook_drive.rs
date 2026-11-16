//! Файл: venues/bsc_dex/exec/hook_drive.rs
//! Что делает: решение «335 (лесенка влияния)», ревью 2–3 — чистая логика пробы крючков B/C на пути сделки (без узла;
//! цикл с узлом — exec/hook_trade.rs, тесты на записях — tests/bsc_dex_hook_ladder_review_1117.rs):
//! - TradeHooks: факты пробы этой сделки (Fresh: точка и влияние, которое расчёт применял на этой сумме до пробы);
//!   curve — лесенка пула для расчёта сделки (check/hook_fold.rs local); лимит hooks.max_impact_* — только по факту
//!   пробы (ревью 3, Н-г: оценка до пробы сделку не режет); uncovered — входы, не покрытые фактом этой сделки;
//! - Drive: шаг цикла. Круг 0 — проба на входах оценки. Вход вырос за точки сделки (точный выход «с конца»: влияние
//!   больше оценки → нужно больше входа) — ревью 3, Б3: следующая проба не на новом входе (он снова вырос бы), а на
//!   входе с поднятым влиянием lift = p + (p − e), где p — факт последней пробы, e — влияние, с которым считался её
//!   вход (рост влияния между пробами берётся вдвое); тогда расчёт по факту даёт вход не больше точки пробы — покрыт.
//!   Подъём — только для выбора суммы пробы: итог сделки — по лесенке сделки без подъёма (факт на ближайшей большей
//!   точке, снятой этой сделкой). Кругов пробы — не больше hooks.impact_points, но не меньше двух (проба на оценке и
//!   проба с подъёмом — сам способ, а не настройка); не задана — два. Не покрыто — отказ этой сделке с причиной.
//!
//! Кто вызывает: exec/hook_trade.rs, exec/live.rs, exec/live_hops.rs (TradeHooks::apply), exec/trade_probe.rs (Fresh).
//! От чего зависит: check/hook_fold.rs (local, covered), quote/select.rs (Usable), core/types (HookStep, PoolRef).
//! Настройки: нет своих (предел кругов передаёт exec/hook_trade.rs).

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
    lift: Vec<(PoolRef, bool, Decimal)>,
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
    fn lift_of(&self, w: &Want) -> Decimal {
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

/// Что делать дальше.
#[derive(Debug, Clone, PartialEq)]
pub enum Next {
    /// Все входы покрыты фактом этой сделки — путь готов.
    Done,
    /// Пересчитать пути (с подъёмом или без).
    Eval,
    /// Проба на этих входах.
    Probe(Vec<Want>),
    /// Отказ этой сделке.
    Fail(String),
}

/// Шаг цикла пробы (см. шапку).
#[derive(Debug, Clone)]
pub struct Drive {
    probes: u64,
    limit: u64,
    lifted: bool,
    /// Ревью 4 (Б5): на каком числе проб подъём уже ставился — второй подъём без новой пробы не ставится.
    lift_used: Option<u64>,
}

impl Drive {
    /// Предел кругов пробы: hooks.impact_points (None — не задана), не меньше двух (см. шапку). Ошибок нет.
    pub fn new(points: Option<u64>) -> Self {
        Drive {
            probes: 0,
            limit: points.unwrap_or(2).max(2),
            lifted: false,
            lift_used: None,
        }
    }

    /// Сколько проб сделано. Ошибок нет.
    pub fn probes(&self) -> u64 {
        self.probes
    }

    /// Что делает: следующий шаг по входам пути после последнего расчёта (см. шапку). Принимает: факты сделки
    /// (подъём ставится и снимается здесь), входы шагов с крючком. Возвращает: Next. Ошибок нет.
    /// Ревью 4 (Б5) — конечность: Done и Fail завершают; Probe увеличивает probes (никогда больше limit: перед каждой
    /// пробой после первой проверено probes < limit); Eval бывает только (а) при постановке подъёма — не больше одного
    /// раза на одно значение probes (lift_used), (б) сразу после расчёта с подъёмом (lifted снимается). Значит, между
    /// двумя ростами probes не больше трёх вызовов, а всего — не больше 3 · (limit + 1) + 1: цикл конечен. Подъём уже
    /// был на этом числе проб, а вход всё равно не покрыт (пул сдвинулся между расчётами) — проба сразу на
    /// непокрытых входах без подъёма.
    pub fn next(&mut self, th: &mut TradeHooks, wants: &[Want]) -> Next {
        if self.lifted {
            self.lifted = false;
            th.lift.clear();
            let need = th.uncovered(wants);
            if need.is_empty() {
                return Next::Eval;
            }
            self.probes += 1;
            return Next::Probe(need);
        }
        let need = th.uncovered(wants);
        if need.is_empty() {
            return Next::Done;
        }
        if self.probes == 0 {
            self.probes = 1;
            return Next::Probe(need);
        }
        if self.probes >= self.limit {
            return Next::Fail(format!(
                "влияние крючка на сумме сделки не покрыто фактом пробы за {} кругов (hooks.impact_points, не меньше \
                 двух) — вход пула растёт с влиянием",
                self.limit
            ));
        }
        if self.lift_used == Some(self.probes) {
            self.probes += 1;
            return Next::Probe(need);
        }
        self.lift_used = Some(self.probes);
        th.lift = need
            .iter()
            .map(|w| (w.pool.clone(), w.buy, th.lift_of(w)))
            .collect();
        self.lifted = true;
        Next::Eval
    }
}
