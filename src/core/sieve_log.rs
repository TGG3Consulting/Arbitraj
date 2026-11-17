//! Файл: core/sieve_log.rs
//! Что делает: решение 374 — итог отсева по границе (sieve.rs) для человека и замера:
//! - отсечённый кандидат: строка «Кандидатов» «Живого экрана» (отказ с причиной и числами, поле `sieve` — граница,
//!   порог, из чего сложилась), эпизод направления закрывается с этой причиной (как отказ расчёта), запись журнала
//!   core.candidate_sieved — одна строка на смену ВИДА причины по направлению (числа в сравнение не входят,
//!   reject_shape.rs), повторы — счётчиком в той же строке (reject_log.rs, частота — journal.repeat_flush_ms): не запись
//!   на каждый отсев. Решение 386: строка в браузер и закрытие эпизода — только когда пишется запись журнала (новый вид
//!   причины — сразу; повтор — с обновлением строки журнала), без таймера (sieve.row_interval_ms удалена);
//! - счётчики с запуска: отсечено (по USDT / по %), пропущено в расчёт, «метод не действует» по классам причин — в
//!   сводку dex.quote_count (quote_count_log.rs, раз в log.quote_count_s), без записи на каждый кандидат;
//! - счётчики контроля (решение 377): audit (взят в контрольный расчёт), audit_violation, audit_moved (sieve_audit.rs).
//!
//! Контроль — sieve_audit.rs (вызывает sieve_gate.rs, не этот файл; от показа не зависит — решение 378).
//! Кто вызывает: core/sieve_gate.rs (count, cut), core/sieve_audit.rs (bump), core/quote_count_log.rs (snapshot).
//! От чего зависит: sieve.rs (Sieve), reject_log.rs, reject_shape.rs, episode_run.rs, view.rs, candidates.rs (reject_key,
//! flush_every).
//! Настройки: journal.repeat_flush_ms (через candidates.rs).

use std::collections::BTreeMap;
use std::sync::{LazyLock, Mutex};

use serde_json::{Value, json};

use crate::core::types::Direction;
use crate::storage::journal::Level;

use super::engine::{Core, lk};
use super::reject_log::{Note, Noted};
use super::sieve::Sieve;
use super::view::CoreEvent;

/// Счётчики с запуска по ключу: cut_usd, cut_pct, pass, na_<класс>.
static COUNTS: LazyLock<Mutex<BTreeMap<String, u64>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

/// Что делает: отмечает итог отсева в счётчиках. Принимает: итог. Ошибок нет.
pub fn count(s: &Sieve) {
    let key = match s {
        Sieve::Cut { data, .. } if data.get("by").and_then(Value::as_str) == Some("pct") => {
            "cut_pct".to_string()
        }
        Sieve::Cut { .. } => "cut_usd".to_string(),
        Sieve::Pass => "pass".to_string(),
        Sieve::Na { class, .. } => format!("na_{class}"),
    };
    bump(&key);
}

/// Что делает: +1 к счётчику по ключу (решение 377: audit, audit_violation, audit_moved). Принимает: ключ. Ошибок нет.
pub fn bump(key: &str) {
    *COUNTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(key.to_string())
        .or_default() += 1;
}

/// Счётчики с запуска (для разницы за интервал сводки). Ошибок нет.
pub fn snapshot() -> BTreeMap<String, u64> {
    COUNTS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Что делает: решение 386 — будет ли этот отсев показан (запись журнала пишется сейчас: RejectLog::due по тому же
/// ключу и виду причины). По нему sieve_gate.rs готовит данные показа (σ-часть, signal_note) только для показа.
/// Принимает: ядро, направление, причину. Возвращает: true — покажется. Ошибок нет.
pub fn due(core: &Core, d: &Direction, reason: &str) -> bool {
    let shape = super::reject_shape::shape(reason);
    core.inner.last_reject.due(
        &super::candidates::reject_key(d),
        &shape,
        &shape,
        super::candidates::flush_every(core),
    )
}

/// Что делает: отсечённый кандидат — журнал, строка «Кандидатов», эпизод (см. шапку). Решение 386: сначала запись
/// журнала тем же механизмом повторов, что core.precheck (reject_log.rs, ключ — направление candidates::reject_key);
/// строка в браузер и закрытие эпизода — только когда запись журнала пишется сейчас: новый вид причины (или первый
/// отсев направления после расчёта) — сразу; повтор того же вида — когда строка журнала обновляется (не чаще
/// journal.repeat_flush_ms; не задана — только на смену вида). Без таймера и своей настройки. Копия данных для строки —
/// только при показе (shown — due до вызова; ревью t171, Н9). Принимает: ядро, направление, причину, данные границы,
/// признак показа. Ошибок нет.
pub async fn cut(core: &Core, d: &Direction, reason: String, data: Value, shown: bool) {
    let shape = super::reject_shape::shape(&reason);
    let pair = format!("{} {} → {}", d.asset.ticker, d.buy_on.0, d.sell_on.0);
    let row_data = shown.then(|| data.clone());
    let note = Note {
        level: Level::Info,
        kind: "core.candidate_sieved",
        venue: Some(&d.buy_on.0),
        // Без чисел и в прежнем режиме (journal.repeat_flush_ms не задана): повтор того же вида — не новая строка.
        exact: shape.clone(),
        shape,
        message: format!("{pair}: {reason}"),
        data: Some(data),
    };
    let flush = super::candidates::flush_every(core);
    let noted = core
        .inner
        .last_reject
        .note(
            &core.inner.deps.journal,
            &super::candidates::reject_key(d),
            note,
            flush,
        )
        .await;
    if !show(noted) {
        return;
    }
    // Гонка due/note (другой вызов между ними) — строка всё равно показывается, без блока «Отсев по границе».
    let row = json!({
        "direction": d,
        "verdict": super::profit::Verdict::Reject { reason: reason.clone(), best: None },
        "at": chrono::Utc::now(),
        "sieve": row_data.unwrap_or(Value::Null),
    });
    {
        let mut rows = lk(&core.inner.candidates);
        let mut row = row;
        if let Some(u) = rows.get(d).and_then(|r| r.get("uncapped")).cloned() {
            row["uncapped"] = u;
        }
        rows.insert(d.clone(), row.clone());
        core.inner.live.send(CoreEvent::Candidate(row));
    }
    super::episode_run::failed(core, d, &reason);
}

/// Что делает: решение 386 — показывать ли отсев (строка в браузер, закрытие эпизода) по итогу записи журнала: да —
/// новая строка или повтор, записанный сейчас; нет — повтор отложен. Чистая. Принимает: итог note. Возвращает: bool.
/// Ошибок нет.
pub fn show(noted: Noted) -> bool {
    !matches!(noted, Noted::Held)
}
