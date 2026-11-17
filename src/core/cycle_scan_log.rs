//! Файл: core/cycle_scan_log.rs
//! Что делает: решение 404 — итог прохода поиска циклов DEX ↔ DEX в журнал core.cycle_scan, свёрнуто: одна строка на
//! вид текста (числа → «#», core/reject_shape.rs), повторы — счётчиком и временем последнего в той же строке, текст и
//! данные — последнего прохода (core/reject_log.rs, частота обновления journal.repeat_flush_ms; не задана — новая
//! строка только при смене вида, числа в ней — первого прохода этого вида). Раньше проход, не давший кандидата, не писал ничего — по
//! журналу нельзя было понять, на каком шаге умирают циклы (docs/notes/t189-dexdex-top-cycles-2026-11-18.md).
//! Две части: (1) строки адаптера по каждой базовой (Venue::dex_cycle_tally — просмотрено, курс круга > 1, отобрано,
//! где отобранные отпали, сигналы); (2) отбор кандидатов ядра (cycle_watch.rs): сигналов, неисполнимых, без чистой
//! прибыли, с чистой ≤ 0, лучшая чистая, кандидатов — с примером причины неисполнимости в данных.
//! Кто вызывает: core/cycle_watch.rs (pass, после поиска циклов).
//! От чего зависит: core/{reject_log, reject_shape, candidates (flush_every), engine}.rs, core/types (DexCycle).
//! Настройки: journal.repeat_flush_ms (через candidates::flush_every).

use rust_decimal::Decimal;
use serde_json::{Value, json};

use crate::core::types::{DexCycle, VenueId};
use crate::storage::journal::Level;

use super::engine::Core;
use super::reject_log::Note;

/// Вид записи журнала итога прохода.
pub const KIND: &str = "core.cycle_scan";

/// Записать итог прохода: строки адаптера и отбор кандидатов ядра. Принимает: ядро, площадку, строки адаптера (текст,
/// данные), сигналы прохода. Ошибок нет.
pub async fn note(core: &Core, v: &VenueId, lines: Vec<(String, Value)>, list: &[DexCycle]) {
    for (i, (text, data)) in lines.into_iter().enumerate() {
        put(core, v, &format!("cycle_scan|{}|{i}", v.0), text, data).await;
    }
    let (text, data) = cands_line(list);
    put(core, v, &format!("cycle_scan|{}|cands", v.0), text, data).await;
}

/// Строка отбора кандидатов ядра (тот же фильтр, что cycle_watch.rs pass: исполним и чистая > 0). Принимает: сигналы.
/// Возвращает: текст и данные (пример причины неисполнимости). Ошибок нет.
pub fn cands_line(list: &[DexCycle]) -> (String, Value) {
    let not_exec: Vec<&DexCycle> = list.iter().filter(|c| !c.executable).collect();
    let exec = || list.iter().filter(|c| c.executable);
    let no_net = exec().filter(|c| c.net_profit.is_none()).count();
    let non_pos = exec()
        .filter(|c| c.net_profit.is_some_and(|p| p <= Decimal::ZERO))
        .count();
    let cands = exec()
        .filter(|c| c.net_profit.is_some_and(|p| p > Decimal::ZERO))
        .count();
    let best = exec().filter_map(|c| c.net_profit).max();
    let best_text = best.map_or_else(
        || "нет".to_string(),
        |b| b.round_dp(8).normalize().to_string(),
    );
    let text = format!(
        "Циклы DEX ↔ DEX, отбор ядра: сигналов {}, неисполнимых {}, исполнимых без чистой прибыли (газ не переведён в \
         базовую) {no_net}, исполнимых с чистой ≤ 0 {non_pos} (лучшая чистая исполнимого {best_text}); кандидатов в \
         план {cands}.",
        list.len(),
        not_exec.len(),
    );
    let data = json!({
        "signals": list.len(),
        "not_executable": not_exec.len(),
        "not_executable_example": not_exec.first().and_then(|c| c.exec_note.clone()),
        "no_net": no_net,
        "net_not_positive": non_pos,
        "best_net": best,
        "candidates": cands,
    });
    (text, data)
}

/// Одна строка через память повторов ядра (reject_log.rs). Ошибок нет.
async fn put(core: &Core, v: &VenueId, key: &str, text: String, data: Value) {
    let deps = &core.inner.deps;
    let shape = super::reject_shape::shape(&text);
    let note = Note {
        level: Level::Info,
        kind: KIND,
        venue: Some(&v.0),
        // Сравнение — по виду (без чисел) и в прежнем режиме: проход идёт на каждый толчок событий пулов, строка на
        // каждый новый набор чисел была бы потоком записей; числа последнего прохода — обновлением строки
        // (journal.repeat_flush_ms).
        shape: shape.clone(),
        exact: shape,
        message: text,
        data: Some(data),
    };
    let every = super::candidates::flush_every(core);
    core.inner
        .last_reject
        .note(&deps.journal, key, note, every)
        .await;
}
