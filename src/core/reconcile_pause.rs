//! Файл: core/reconcile_pause.rs
//! Что делает: «Пауза» системы от сверки балансов и её автоснятие (решение 221, Тигран 08.11.2026). Случай 08.11:
//! сверка в 00:46:53 UTC поставила «Паузу» (PORTO по учёту 119.25, на площадке 0 — человек продал вручную до кнопки
//! «Беру на себя»); в 00:47:18 / 00:48:18 сделки закрыли кнопками, расхождение исчезло, а пауза висела до ручного
//! «Старт» в 07:31:15, и в журнале не было строки о смене режима. Теперь:
//! (1) пауза от сверки пишется в журнал (core.control, как остальные смены режима) — режим, причина и сколько и по
//!     чему расхождение; запоминается режим до паузы (Risk.reconcile_pause);
//! (2) если на следующих кругах сверки расхождений нет ledger.reconcile_confirm_rounds кругов подряд (та же защита,
//!     что для постановки; не задано или 0 — один круг) — режим до паузы возвращается, журнал core.control и
//!     уведомление risk.reconcile_resumed. Паузу человека или другого модуля и паузу, после которой режим меняли
//!     (любая смена режима снимает отметку — risk::set_mode, daily_limit.rs), сверка не трогает.
//! «Расхождений нет» — в круге нет ни одной проблемы risk.reconcile_mismatch (ни подтверждённой, ни ждущей кругов, ни
//! «балансы не получены»): не сверили — не знаем, паузу не снимаем.
//! Кто вызывает: core/reconcile.rs (run).
//! От чего зависит: engine.rs, risk.rs, reconcile.rs (Found), reconcile_confirm.rs (is_unavailable), settings.
//! Настройки: ledger.reconcile_confirm_rounds.

use crate::settings::read::At;
use crate::storage::journal::Level;

use super::engine::{Core, lk};
use super::reconcile::Found;
use super::reconcile_confirm::is_unavailable;
use super::risk::Mode;

/// Код события расхождения сверки.
const MISMATCH: &str = "risk.reconcile_mismatch";

/// Счётчик чистых кругов подряд, пока стоит пауза сверки (чистая логика, без ядра).
#[derive(Default)]
pub struct Lift {
    clean: u64,
}

impl Lift {
    /// Отмечает круг. Принимает: стоит ли сейчас пауза, поставленная сверкой; чистый ли круг; сколько чистых кругов
    /// подряд нужно (меньше 1 — как 1). Возвращает: true — пора снять паузу (счётчик обнуляется). Ошибок нет.
    pub fn round(&mut self, paused_by_reconcile: bool, clean: bool, need: u64) -> bool {
        if !paused_by_reconcile || !clean {
            self.clean = 0;
            return false;
        }
        self.clean += 1;
        if self.clean >= need.max(1) {
            self.clean = 0;
            return true;
        }
        false
    }

    /// Чистых кругов подряд сейчас. Ошибок нет.
    pub fn clean(&self) -> u64 {
        self.clean
    }
}

/// Круг чистый: ни одной проблемы risk.reconcile_mismatch среди всех найденных (до подтверждения кругами).
/// Принимает: все проблемы круга. Ошибок нет.
pub fn is_clean(all: &[Found]) -> bool {
    !all.iter().any(|f| f.0 == MISMATCH)
}

/// Расхождения, ставящие системную паузу (подтверждённые, не «балансы не получены»), текстом через «; ».
/// Пусто — паузы нет. Ошибок нет.
pub fn mismatch_text(found: &[Found]) -> String {
    found
        .iter()
        .filter(|f| f.0 == MISMATCH && !is_unavailable(f))
        .map(|f| f.2.as_str())
        .collect::<Vec<_>>()
        .join("; ")
}

/// Ставит «Паузу» от сверки, если система в работе и есть подтверждённое расхождение; журнал core.control.
/// Принимает: ядро, подтверждённые проблемы круга. Ошибок нет.
pub async fn pause(core: &Core, found: &[Found]) {
    let what = mismatch_text(found);
    if what.is_empty() {
        return;
    }
    let reason = "расхождение при сверке балансов".to_string();
    let was = {
        let mut r = lk(&core.inner.risk);
        if r.mode != Mode::Running {
            return;
        }
        let was = r.mode;
        r.mode = Mode::Paused;
        r.reason = Some(reason.clone());
        r.daily_stop = false;
        r.reconcile_pause = Some(was);
        was
    };
    super::risk::publish(core);
    let text = format!(
        "Режим системы: {:?} → Paused — {reason}: {what}. Снимется сам, когда расхождений не будет \
         ledger.reconcile_confirm_rounds кругов сверки подряд (решение 221)",
        was
    );
    core.inner
        .deps
        .log(Level::Info, "core.control", None, &text, None)
        .await;
}

/// Круг сверки для автоснятия: чистый круг при паузе сверки считается; набралось кругов — режим до паузы
/// возвращается (только если пауза всё ещё сверки — отметка не снята сменой режима), журнал и risk.reconcile_resumed.
/// Принимает: ядро, счётчик, все проблемы круга. Ошибок нет.
pub async fn maybe_lift(core: &Core, lift: &mut Lift, all: &[Found]) {
    let ours = lk(&core.inner.risk).reconcile_pause.is_some();
    let need = core
        .inner
        .deps
        .settings
        .integer("ledger.reconcile_confirm_rounds", At::default())
        .unwrap_or(0)
        .max(1);
    if !lift.round(ours, is_clean(all), need) {
        return;
    }
    let back = {
        let mut r = lk(&core.inner.risk);
        match r.reconcile_pause.take() {
            Some(prev) if r.mode == Mode::Paused => {
                r.mode = prev;
                r.reason = Some(format!(
                    "пауза сверки снята сама: расхождений нет {need} кругов подряд"
                ));
                Some(prev)
            }
            _ => None,
        }
    };
    let Some(prev) = back else { return };
    super::risk::publish(core);
    let text = format!(
        "Режим системы: Paused → {prev:?} — пауза от сверки балансов снята сама: расхождений нет {need} кругов \
         сверки подряд (решение 221)"
    );
    let deps = &core.inner.deps;
    deps.log(Level::Info, "core.control", None, &text, None)
        .await;
    deps.notify("risk.reconcile_resumed", None, &text).await;
}
