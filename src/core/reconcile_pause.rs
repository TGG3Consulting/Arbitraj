//! Файл: core/reconcile_pause.rs
//! Что делает: пауза ПЛОЩАДКИ от сверки балансов и её автоснятие (решение 221, Тигран 08.11.2026). Случай 08.11:
//! сверка в 00:46:53 UTC поставила «Паузу» всей системы (PORTO по учёту 119.25, на площадке 0 — человек продал вручную
//! до кнопки «Беру на себя»); расхождение исчезло через минуту, а пауза висела до ручного «Старт» в 07:31:15.
//! Тигран (08.11): «Пауза ложится всегда только на биржу, где проблема, и сама снимается, если уже всё ок. Но если
//! паузу поставили вручную — снимается только вручную». Теперь:
//! (1) подтверждённая проблема сверки площадки (монет меньше, чем по учёту, или балансы не получены — после
//!     ledger.reconcile_confirm_rounds кругов) ставит паузу только ЭТОЙ площадке (Risk.venue_paused — тот же механизм,
//!     что у ошибок подряд и правила шлюза StopVenue), если она ещё не на паузе; отметка «паузу поставила сверка» —
//!     Risk.reconcile_venues; журнал core.control с площадкой, причиной и суммами. Системный режим сверка не меняет.
//!     У DEX пауза — площадки (например bsc_dex) целиком: пауза по кошельку не поддерживается (venue_paused — по id
//!     площадки), поэтому расхождение одного кошелька останавливает все кошельки этой DEX;
//! (2) если по ЭТОЙ площадке ledger.reconcile_confirm_rounds кругов подряд чистые (площадка сверялась в круге — все её
//!     балансы получены — и нет ни одной её проблемы risk.reconcile_mismatch, ни подтверждённой, ни ждущей кругов;
//!     не задано / 0 / 1 — один круг) — пауза площадки снимается, журнал core.control и уведомление
//!     risk.reconcile_resumed. Проблемы и недоступность ДРУГИХ площадок не мешают;
//! (3) паузу площадки, которую поставил или снял человек (risk::resume_venue) или другой модуль (risk::on_error,
//!     правило шлюза, «исход неизвестен» — rules_apply.rs, лимит дня — daily_venue_limit.rs), сверка не трогает: каждый
//!     из них снимает отметку (risk::forget_reconcile), и автоснятия сверкой нет. Решение 225: пауза ошибок подряд,
//!     правила «стоп площадка» и «исход неизвестен» поверх паузы сверки переходит в отметку auto_pause (Risk.venue_auto,
//!     auto_pause::pause_venue снимает отметку сверки): дальше её снимает auto_resume.rs, «исход неизвестен» — человек;
//!     площадку с «исход неизвестен» (auto_pause::human_only) сверка не снимает никогда.
//! Кто вызывает: core/reconcile.rs (run).
//! От чего зависит: engine.rs, risk.rs, reconcile.rs (Found), settings.
//! Настройки: ledger.reconcile_confirm_rounds.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::core::types::VenueId;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::engine::{Core, lk};
use super::reconcile::Found;

/// Код события расхождения сверки.
const MISMATCH: &str = "risk.reconcile_mismatch";

/// Счётчик чистых кругов подряд по площадкам, пока на них стоит пауза сверки (чистая логика, без ядра).
#[derive(Default)]
pub struct Lift {
    clean: HashMap<String, u64>,
}

impl Lift {
    /// Отмечает круг площадки. Принимает: площадку; стоит ли сейчас на ней пауза, поставленная сверкой; чистый ли
    /// круг этой площадки; сколько чистых кругов подряд нужно (меньше 1 — как 1). Возвращает: true — пора снять паузу
    /// (счётчик обнуляется). Ошибок нет.
    pub fn round(
        &mut self,
        venue: &str,
        paused_by_reconcile: bool,
        clean: bool,
        need: u64,
    ) -> bool {
        if !paused_by_reconcile || !clean {
            self.clean.remove(venue);
            return false;
        }
        let n = self.clean.entry(venue.to_string()).or_default();
        *n += 1;
        if *n >= need.max(1) {
            self.clean.remove(venue);
            return true;
        }
        false
    }

    /// Забывает площадки, на которых паузы сверки больше нет. Принимает: площадки с отметкой. Ошибок нет.
    pub fn retain(&mut self, marked: &HashSet<String>) {
        self.clean.retain(|v, _| marked.contains(v));
    }

    /// Чистых кругов подряд сейчас у площадки. Ошибок нет.
    pub fn clean(&self, venue: &str) -> u64 {
        self.clean.get(venue).copied().unwrap_or(0)
    }
}

/// Круг площадки чистый: площадка сверялась в этом круге и среди всех проблем круга (до подтверждения кругами) нет
/// ни одной её проблемы risk.reconcile_mismatch. Принимает: площадку, сверенные в круге площадки, все проблемы круга.
/// Ошибок нет.
pub fn is_clean(venue: &str, checked: &HashSet<String>, all: &[Found]) -> bool {
    checked.contains(venue)
        && !all
            .iter()
            .any(|f| f.0 == MISMATCH && f.1.as_deref() == Some(venue))
}

/// Подтверждённые проблемы сверки по площадкам (ставят паузу площадки): площадка → тексты через «; ».
/// Принимает: подтверждённые проблемы круга. Ошибок нет.
pub fn mismatch_by_venue(found: &[Found]) -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for f in found.iter().filter(|f| f.0 == MISMATCH) {
        if let Some(v) = &f.1 {
            out.entry(v.clone()).or_default().push(f.2.as_str());
        }
    }
    out.into_iter().map(|(v, t)| (v, t.join("; "))).collect()
}

/// Ставит паузу площадке с подтверждённой проблемой сверки, если она ещё не на паузе; отметка сверки, журнал
/// core.control. Площадку на паузе по другой причине не трогает. Принимает: ядро, подтверждённые проблемы круга.
/// Ошибок нет.
pub async fn pause(core: &Core, found: &[Found]) {
    let need = need(core);
    for (v, what) in mismatch_by_venue(found) {
        let id = VenueId(v.clone());
        let fresh = {
            let mut r = lk(&core.inner.risk);
            if r.venue_paused.contains_key(&id) {
                false
            } else {
                r.venue_paused
                    .insert(id.clone(), format!("сверка балансов: {what}"));
                r.reconcile_venues.insert(id);
                true
            }
        };
        if !fresh {
            continue;
        }
        super::risk::publish(core);
        let text = format!(
            "Пауза площадки {v} — расхождение при сверке балансов: {what}. Снимется сама, когда по площадке \
             расхождений не будет {need} кругов сверки подряд (решение 221)"
        );
        core.inner
            .deps
            .log(Level::Info, "core.control", Some(&v), &text, None)
            .await;
    }
}

/// Круг сверки для автоснятия: по каждой площадке с паузой сверки чистый круг считается; набралось кругов подряд —
/// пауза площадки снимается (только если отметка сверки ещё стоит), журнал core.control и risk.reconcile_resumed.
/// Принимает: ядро, счётчик, сверенные в круге площадки, все проблемы круга. Ошибок нет.
pub async fn maybe_lift(core: &Core, lift: &mut Lift, checked: &HashSet<String>, all: &[Found]) {
    let marked: HashSet<String> = lk(&core.inner.risk)
        .reconcile_venues
        .iter()
        .map(|v| v.0.clone())
        .collect();
    lift.retain(&marked);
    let need = need(core);
    for v in marked {
        if !lift.round(&v, true, is_clean(&v, checked, all), need) {
            continue;
        }
        let id = VenueId(v.clone());
        let lifted = {
            let mut r = lk(&core.inner.risk);
            // Решение 225: «исход неизвестен» снимает только человек — сверка такую площадку не снимает (отметка сверки
            // при этом уже снята auto_pause::pause_venue; проверка — страховка).
            !super::auto_pause::human_only(&r, &id)
                && r.reconcile_venues.remove(&id)
                && r.venue_paused.remove(&id).is_some()
        };
        if !lifted {
            continue;
        }
        super::risk::publish(core);
        let text = format!(
            "Пауза площадки {v} от сверки балансов снята сама: расхождений нет {need} кругов сверки подряд \
             (решение 221)"
        );
        let deps = &core.inner.deps;
        deps.log(Level::Info, "core.control", Some(&v), &text, None)
            .await;
        deps.notify("risk.reconcile_resumed", Some(&v), &text).await;
    }
}

/// Сколько кругов подряд нужно (ledger.reconcile_confirm_rounds; не задано или 0 — 1). Ошибок нет.
fn need(core: &Core) -> u64 {
    core.inner
        .deps
        .settings
        .integer("ledger.reconcile_confirm_rounds", At::default())
        .unwrap_or(0)
        .max(1)
}
