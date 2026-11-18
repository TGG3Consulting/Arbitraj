//! Файл: core/manual_wait_mark.rs
//! Что делает: решение 408, ответ Тиграна 18.11 на вопрос 9а — ожидание ручной кнопки (решение 405, manual_wait.rs)
//! живёт в памяти и после перезапуска не продолжается; чтобы человек знал, что его нажатие пропало, в базе хранится
//! только ОТМЕТКА «ожидание было» (что, монета, кто, когда и кнопка, которую нажимали) — БЕЗ команды:
//! - начало ожидания (manual_wait_run.rs start) — отметка в core_state (вид manual_wait_mark);
//! - конец ожидания любым путём (выполнено, отменено, отказ) — отметка снимается;
//! - запуск (engine.rs, после загрузки учёта) — по каждой оставшейся отметке сообщение manual.wait_lost «ожидание X по
//!   монете Y потеряно при перезапуске» с той же кнопкой (кнопка записи «Ручного управления» — по текущим записям
//!   экрана; «Продать» «Балансов» — e:sb), затем отметка снимается.
//!
//! Кнопка: запись экрана с этой командой (ManualRecord::matches) и её кнопка того же вида — «<код>:<ключ записи>»;
//! «Продать» «Балансов» — e:sb.<биржа>.<монета>; иначе кнопки нет (в тексте — где нажать).
//! Кто вызывает: manual_wait_run.rs (save, forget), engine.rs (lost).
//! От чего зависит: persist.rs (core_state), manual_wait.rs (ManualWait), manual_feed.rs (manual_records),
//! notify (callback::encode, acts — кнопка, deps.notify_acts).
//! Настройки: нет (событие manual.wait_lost — notify.event.*).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::core::types::{ManualAction, ManualCommand};
use crate::notify::acts::{Act, act};
use crate::storage::journal::Level;

use super::engine::{Core, lk};

/// Вид записи core_state.
pub const KIND: &str = "manual_wait_mark";
/// Код события уведомлений (notify/catalog.rs).
pub const EVENT: &str = "manual.wait_lost";

/// Отметка «ожидание было» — без команды.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mark {
    pub id: String,
    /// Что нажато (надпись кнопки).
    pub what: String,
    pub coin: String,
    pub who: String,
    pub since: DateTime<Utc>,
    /// Кнопка, которую нажимали: (надпись, callback_data). None — нажато не кнопкой записи и не «Продать».
    #[serde(default)]
    pub button: Option<(String, String)>,
}

/// Кнопка нажатой команды (см. шапку). Принимает: ядро, команду. Ошибок нет.
fn button(core: &Core, cmd: &ManualCommand) -> Option<(String, String)> {
    if let ManualAction::SellBalance { venue, asset, .. } = &cmd.action {
        let a = crate::notify::acts::e::sell(venue, asset);
        return Some((a.title, a.data));
    }
    let want = std::mem::discriminant(&cmd.action);
    core.manual_records().into_iter().find_map(|r| {
        if !r.matches(&cmd.action) {
            return None;
        }
        let b = r
            .buttons
            .iter()
            .find(|b| std::mem::discriminant(&b.action) == want)?;
        let data = crate::notify::callback::encode(&b.code, &r.key).ok()?;
        Some((b.title.clone(), data))
    })
}

/// Что делает: отметка начала ожидания. Принимает: ядро, ключ монеты, ID ожидания, команду. Ошибок нет: сбой базы —
/// журнал core.manual_wait.
pub async fn save(core: &Core, key: &super::coin_key::CoinKey, id: &str, cmd: &ManualCommand) {
    let Some(w) = lk(&core.inner.manual_waits).get(key).cloned() else {
        return;
    };
    let m = Mark {
        id: id.to_string(),
        what: w.what,
        coin: w.coin,
        who: w.who,
        since: w.since,
        button: button(core, cmd),
    };
    if let Err(e) = super::persist::save(&core.inner.deps.db, KIND, id, &m).await {
        let t = format!("отметка ожидания {id} не сохранена: {e}");
        super::manual_wait::log(core, Level::Error, &t).await;
    }
}

/// Что делает: снять отметку (ожидание кончилось). Принимает: ядро, ID. Ошибок нет: сбой — журнал.
pub async fn forget(core: &Core, id: &str) {
    if let Err(e) = super::persist::remove(&core.inner.deps.db, KIND, id).await {
        let t = format!("отметка ожидания {id} не снята: {e}");
        super::manual_wait::log(core, Level::Error, &t).await;
    }
}

/// Что делает: при запуске — сообщение «потеряно при перезапуске» по каждой отметке прошлого запуска и снятие отметки
/// (после постановки сообщения в очередь уведомлений). Принимает: ядро. Ошибок нет: всё — в журнал.
pub async fn lost(core: &Core) {
    let list: Vec<Mark> = match super::persist::load(&core.inner.deps.db, KIND).await {
        Ok(v) => v,
        Err(e) => {
            let t = format!("отметки ожиданий не прочитаны: {e}");
            return super::manual_wait::log(core, Level::Error, &t).await;
        }
    };
    for m in list {
        let text = format!(
            "Ожидание «{}» по монете {} (нажал: {}, с {}) потеряно при перезапуске: действие не выполнено. {}",
            m.what,
            m.coin,
            m.who,
            m.since.format("%Y-%m-%d %H:%M:%S UTC"),
            if m.button.is_some() {
                "Нажмите кнопку снова"
            } else {
                "Нажмите снова на экране «Ручное управление» или «Балансы»"
            }
        );
        super::manual_wait::log(core, Level::Warn, &text).await;
        let acts: Vec<Act> = m.button.iter().map(|(t, d)| act(t, d)).collect();
        core.inner.deps.notify_acts(EVENT, None, &text, acts).await;
        forget(core, &m.id).await;
    }
}
