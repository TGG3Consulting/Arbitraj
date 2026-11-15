//! Файл: core/fit_settings.rs
//! Что делает: решение 294 — чтение необязательного числа попыток уменьшения объёма: signal.fit_halvings (сигнал,
//! fit_volume.rs) и size.final_fit_steps (финальный расчёт, final_fit.rs). Не задана — None (уменьшения нет, как
//! раньше). Значение в неверном виде (не целое, меньше 1, больше 4294967295) — тоже None (уменьшения нет, остальной
//! расчёт идёт как раньше) и одна запись Warn в журнал «core.fit_volume» на (настройка, причина) за запуск: сигнал
//! пересчитывается на каждом тике — запись на каждый пересчёт засорила бы журнал.
//! Кто вызывает: core/fit_volume.rs (retry), core/final_fit.rs (fill), тесты (read).
//! От чего зависит: settings (Store::integer), deps.rs (журнал).
//! Настройки: signal.fit_halvings, size.final_fit_steps (вся система; необязательные).

use std::collections::HashSet;
use std::sync::Mutex;

use crate::gateway::errors::ErrorCode;
use crate::settings::read::At;
use crate::settings::store::Store;
use crate::storage::journal::Level;

use super::engine::Core;

/// Вид записи журнала.
pub const KIND: &str = "core.fit_volume";

/// Причины, о которых уже записано (одна запись на «настройка: причина» за запуск).
static LOGGED: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// Что делает: читает число попыток. Принимает: хранилище, имя настройки.
/// Возвращает: Some(n) (n ≥ 1) или None — не задана. Ошибки: текст — значение в неверном виде (в т.ч. больше u32).
pub fn read(s: &Store, name: &str) -> Result<Option<u32>, String> {
    match s.integer(name, At::default()) {
        Ok(v) => match u32::try_from(v) {
            Ok(n) if n > 0 => Ok(Some(n)),
            _ => Err(format!("{name} = {v}: нужно целое от 1 до {}", u32::MAX)),
        },
        Err(e) if e.code == ErrorCode::NotConfigured => Ok(None),
        Err(e) => Err(format!("{name}: {}", e.message)),
    }
}

/// Что делает: число попыток для расчёта. Принимает: ядро, имя настройки. Возвращает: Some(n) или None — не задана
/// или в неверном виде (тогда — одна запись Warn за запуск на причину). Ошибок нет.
pub async fn tries(core: &Core, name: &str) -> Option<u32> {
    match read(&core.inner.deps.settings, name) {
        Ok(n) => n,
        Err(why) => {
            let fresh = {
                let mut g = LOGGED.lock().unwrap_or_else(|p| p.into_inner());
                g.get_or_insert_with(HashSet::new).insert(why.clone())
            };
            if fresh {
                let text = format!("{why} — уменьшение объёма не выполняется (решение 294)");
                core.inner
                    .deps
                    .log(Level::Warn, KIND, None, text, None)
                    .await;
            }
            None
        }
    }
}
