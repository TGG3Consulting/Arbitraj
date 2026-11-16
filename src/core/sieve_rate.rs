//! Файл: core/sieve_rate.rs
//! Что делает: ревью t162, Н4 — предел частоты строки отсева по направлению. Отсев занимает микросекунды, поэтому
//! «кандидат», который держится на каждом тике, отсекался бы на каждом тике: событие строки «Кандидатов» в браузер,
//! запись повтора в журнал (core.candidate_sieved), закрытие эпизода. На контроль (sieve.audit_share) предел не влияет — решение 378.
//! Теперь всё это — не чаще раза в sieve.row_interval_ms по направлению; в промежутке отсев только считается (счётчики
//! сводки dex.quote_count). Безопасно: в промежутке по направлению не было ничего, кроме отсева (любой полный расчёт
//! направления снимает отметку — clear), значит строка, эпизод и журнал уже показывают отсев (числа — последнего
//! показанного). Не задана или 0 — каждый отсев показывается (как раньше).
//! Кто вызывает: core/sieve_gate.rs.
//! От чего зависит: engine.rs (настройки), core/types (Direction).
//! Настройки: sieve.row_interval_ms.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use crate::core::types::Direction;
use crate::settings::read::At;

use super::engine::Core;

/// Когда по направлению последний раз показан отсев.
static LAST: LazyLock<Mutex<HashMap<Direction, Instant>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Что делает: показать ли этот отсев (и отметить показ). Принимает: ядро, направление. Возвращает: true — показать
/// (интервал не задан, 0, первый отсев направления или интервал прошёл). Ошибок нет.
pub fn due(core: &Core, d: &Direction) -> bool {
    let every = core
        .inner
        .deps
        .settings
        .integer("sieve.row_interval_ms", At::default())
        .ok()
        .filter(|ms| *ms > 0)
        .map(Duration::from_millis);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    if let (Some(every), Some(at)) = (every, last.get(d))
        && now.duration_since(*at) < every
    {
        return false;
    }
    last.insert(d.clone(), now);
    true
}

/// Что делает: по направлению идёт полный расчёт — отметка снимается (следующий отсев покажется сразу). Принимает:
/// направление. Ошибок нет.
pub fn clear(d: &Direction) {
    LAST.lock().unwrap_or_else(|e| e.into_inner()).remove(d);
}
