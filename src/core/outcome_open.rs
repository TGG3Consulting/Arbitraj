//! Файл: core/outcome_open.rs
//! Что делает: решение 231 — учёт открытых «исход неизвестен» по площадке (чистая логика над Risk, без ядра).
//! Пауза площадки с отметкой Outcome (auto_pause.rs) снимается ботом только тогда, когда ВСЕ операции, поставившие её,
//! выяснены ботом (core/outcome_resolve.rs): у каждой — ключ (свой ID ордера или вывода, gateway/pending.rs). Операция
//! без ключа (адаптер не приложил «чем найти», выяснение не задано настройками, цикл DEX ↔ DEX) или не выясненная за
//! risk.outcome_max_s — «только человек»: такую паузу бот не снимает. Отказ остановленной очереди кошелька сюда больше
//! не попадает (решение 231, дополнение: код QueueHalted — «не отправлено», обычные правила шлюза).
//! Пауза, стоявшая до «исход неизвестен» (человек, правило, ошибки подряд — не сверка: её отметку pause_venue снимает),
//! запоминается и при снятии возвращается как была; пауза по убытку дня (Risk.loss_paused) остаётся (снимет day_roll.rs).
//! «Снять паузу» человеком (risk::resume_venue) снимает и этот учёт.
//! Кто вызывает: rules_stop.rs (register), outcome_resolve.rs (resolving, resolved, unresolved), risk.rs (resume_venue),
//! тесты (tests/core_outcome_resolve.rs).
//! От чего зависит: risk.rs (Risk), auto_pause.rs (VenueMark, Kind).
//! Настройки: нет.

use std::collections::BTreeSet;

use crate::core::types::VenueId;

use super::auto_pause::{Kind, VenueMark};
use super::risk::Risk;

/// Открытые «исход неизвестен» одной площадки.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Open {
    /// Ключи операций, исход которых бот выясняет сам.
    pub resolving: BTreeSet<String>,
    /// Есть операция, исход которой выясняет только человек.
    pub manual: bool,
    /// Пауза площадки до первого «исход неизвестен»: причина и софтовая отметка (None — паузы не было).
    pub prior: Option<(String, Option<VenueMark>)>,
}

/// Чем кончилось выяснение одной операции.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lift {
    /// Пауза «исход неизвестен» снята (вернулась прежняя пауза, если была).
    Lifted,
    /// Пауза остаётся: ещё выясняются операции (сколько) или есть операция «только человек».
    Kept { left: usize, manual: bool },
    /// Учёта нет: паузу уже снял человек.
    Gone,
}

/// Пауза площадки до «исход неизвестен» — вызывать ДО auto_pause::pause_venue. Пауза сверки и уже стоящая отметка
/// Outcome прежней не считаются. Принимает: состояние риска, площадку. Возвращает: (причина, отметка) или None.
pub fn prior(r: &Risk, v: &VenueId) -> Option<(String, Option<VenueMark>)> {
    if r.reconcile_venues.contains(v) || super::auto_pause::human_only(r, v) {
        return None;
    }
    let why = r.venue_paused.get(v)?.clone();
    Some((why, r.venue_auto.get(v).cloned()))
}

/// Новая операция «исход неизвестен» на площадке — вызывать после auto_pause::pause_venue под той же блокировкой.
/// Принимает: состояние риска, площадку, ключ (None — выясняет только человек), прежнюю паузу (prior()). Ошибок нет.
pub fn register(
    r: &mut Risk,
    v: &VenueId,
    key: Option<&str>,
    prior: Option<(String, Option<VenueMark>)>,
) {
    let o = r.outcome_open.entry(v.clone()).or_insert_with(|| Open {
        prior,
        ..Open::default()
    });
    match key.filter(|k| !k.is_empty()) {
        Some(k) => {
            o.resolving.insert(k.to_string());
        }
        None => o.manual = true,
    }
}

/// Выясняет ли бот исход этой операции (зарегистрирована с ключом и человек паузу не снимал). Ошибок нет.
pub fn resolving(r: &Risk, v: &VenueId, key: &str) -> bool {
    r.outcome_open
        .get(v)
        .is_some_and(|o| o.resolving.contains(key))
}

/// Исход операции выяснен: ключ снят; больше открытых нет и нет «только человек», а отметка всё ещё Outcome — пауза
/// снимается (прежняя возвращается; по убытку дня — остаётся без отметки). Принимает: состояние риска, площадку, ключ.
/// Возвращает: Lift. Ошибок нет.
pub fn resolved(r: &mut Risk, v: &VenueId, key: &str) -> Lift {
    let Some(o) = r.outcome_open.get_mut(v) else {
        return Lift::Gone;
    };
    o.resolving.remove(key);
    if !o.resolving.is_empty() || o.manual {
        return Lift::Kept {
            left: o.resolving.len(),
            manual: o.manual,
        };
    }
    let prior = o.prior.take();
    r.outcome_open.remove(v);
    if !r.venue_auto.get(v).is_some_and(|m| m.kind == Kind::Outcome) {
        return Lift::Gone;
    }
    r.venue_auto.remove(v);
    match prior {
        Some((why, mark)) => {
            r.venue_paused.insert(v.clone(), why);
            if let Some(m) = mark {
                r.venue_auto.insert(v.clone(), m);
            }
        }
        // Пауза по убытку дня поставлена поверх — остаётся (её снимет новый день, day_roll.rs).
        None if r.loss_paused.contains(&v.0) => {}
        None => {
            r.venue_paused.remove(v);
            r.errors_in_row.remove(v);
        }
    }
    Lift::Lifted
}

/// Исход не выяснен за срок (или выяснять нельзя): операция становится «только человек». Принимает: состояние риска,
/// площадку, ключ. Ошибок нет.
pub fn unresolved(r: &mut Risk, v: &VenueId, key: &str) {
    if let Some(o) = r.outcome_open.get_mut(v) {
        o.resolving.remove(key);
        o.manual = true;
    }
}
