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

use std::sync::OnceLock;

use axum::Json;
use serde::{Deserialize, Serialize};

use super::state::ApiError;

/// Справочник встроен в бинарник: всегда доступен и совпадает с версией сборки (путь — от каталога этого файла).
const FIRST_RUN_TOML: &str = include_str!("../../config/first_run.toml");

/// Куда интерфейс может ПРЕДзаполнить значение на экране «Настройки» (только подсказка форме; записи нет).
/// kind — область значения («Global» / «Venue»); id — для «Venue» (например «bsc_dex», «bingx»).
#[derive(Deserialize, Serialize, Clone)]
pub struct Prefill {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

/// Одна строка таблицы. value — готовое советуемое значение (текст); method — метод/формула, когда готового числа
/// быть не должно (капитал-зависимые money.*/risk.* — подбирает Тигран). source — чем обосновано (док/решение).
#[derive(Deserialize, Serialize, Clone)]
pub struct Rec {
    pub name: String,
    pub area: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub rationale: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefill: Option<Prefill>,
}

/// Раздел таблицы (поле rec — строки раздела).
#[derive(Deserialize, Serialize, Clone)]
pub struct Section {
    pub title: String,
    pub intro: String,
    #[serde(default)]
    pub rec: Vec<Rec>,
}

/// Вся таблица. advisory всегда true — значения советуемые, сохраняет их Тигран сам.
#[derive(Deserialize, Serialize, Clone)]
pub struct FirstRun {
    pub advisory: bool,
    pub note: String,
    pub source_note: String,
    #[serde(default)]
    pub section: Vec<Section>,
}

/// Разбирает встроенный справочник один раз (ошибка разбора — только при порче файла в сборке).
fn parsed() -> &'static Result<FirstRun, String> {
    static PARSED: OnceLock<Result<FirstRun, String>> = OnceLock::new();
    PARSED.get_or_init(|| toml::from_str::<FirstRun>(FIRST_RUN_TOML).map_err(|e| e.to_string()))
}

/// Таблица первого запуска. Ошибки: 500 — встроенный config/first_run.toml не разобрался (порча сборки).
pub async fn table() -> Result<Json<&'static FirstRun>, ApiError> {
    match parsed() {
        Ok(fr) => Ok(Json(fr)),
        Err(e) => Err(ApiError::internal(format!("config/first_run.toml: {e}"))),
    }
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
