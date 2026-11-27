//! Файл: api/first_run.rs
//! Что делает: отдаёт интерфейсу «Таблицу первого запуска» — СОВЕТУЕМЫЕ стартовые значения настроек.
//!   Это справочные ДАННЫЕ (config/first_run.toml), а не значения по умолчанию движка и не запись настроек:
//!   движок по-прежнему без стартовых значений (CLAUDE.md, docs/SETTINGS.md). Эндпоинт ТОЛЬКО читает справочник
//!   и возвращает его как есть — сохраняет настройки Тигран сам на экране «Настройки» (PUT /api/settings, с журналом).
//! Кто вызывает: интерфейс (экран «Первый запуск», ui/src/pages/FirstRun.svelte).
//! От чего зависит: config/first_run.toml (встроен в бинарник include_str!), api/state.rs (ApiError), toml, serde.
//! Настройки: своих нет; строки таблицы ссылаются на уже существующие настройки docs/SETTINGS.md, новых не добавляет.
//!
//! GET /api/first-run — {advisory, note, source_note, section: [{title, intro, rec: [Rec]}]}

use axum::{Json, extract::State};

use super::state::{ApiError, AppState};
use crate::settings::recommendations::parsed;
pub use crate::settings::recommendations::{FirstRun, Prefill, Rec, Section};

/// Таблица первого запуска. Ошибки: 500 — встроенный config/first_run.toml не разобрался (порча сборки).
pub async fn table(State(st): State<AppState>) -> Result<Json<FirstRun>, ApiError> {
    let mut fr = parsed()
        .as_ref()
        .map_err(|e| ApiError::internal(format!("config/first_run.toml: {e}")))?
        .clone();
    fr.section
        .extend(super::first_run_dex::sections(&st).await?);
    Ok(Json(fr))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Встроенный справочник разбирается, он помечен советуемым и каждая строка несёт либо value, либо method
    /// (строка без готового значения обязана объяснять метод). Проверка на реальном файле, без моков.
    #[test]
    fn embedded_table_parses_and_is_advisory() {
        let fr = parsed()
            .as_ref()
            .expect("config/first_run.toml разбирается");
        assert!(fr.advisory, "таблица помечена советуемой");
        assert!(!fr.section.is_empty(), "в таблице есть разделы");
        for s in &fr.section {
            for r in &s.rec {
                assert!(
                    r.value.is_some() || r.method.is_some(),
                    "у строки {} должно быть value или method",
                    r.name
                );
            }
        }
    }
}
