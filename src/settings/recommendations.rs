//! Поставляемая таблица первого запуска. UI показывает её значения, клиент сверяет с ними адрес получателя ключа.
//! Читается один раз из config/first_run.toml этой сборки, отдельно от редактируемого хранилища.

use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

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
pub(crate) fn parsed() -> &'static Result<FirstRun, String> {
    static PARSED: OnceLock<Result<FirstRun, String>> = OnceLock::new();
    PARSED.get_or_init(|| toml::from_str::<FirstRun>(FIRST_RUN_TOML).map_err(|e| e.to_string()))
}

/// Возвращает единственную встроенную рекомендацию указанного параметра для площадки.
pub(crate) fn venue_value(name: &str, venue: &str) -> Option<&'static str> {
    let mut rows = parsed()
        .as_ref()
        .ok()?
        .section
        .iter()
        .flat_map(|s| &s.rec)
        .filter(|r| {
            r.name == name
                && r.prefill
                    .as_ref()
                    .is_some_and(|p| p.kind == "Venue" && p.id.as_deref() == Some(venue))
        });
    let first = rows.next()?.value.as_deref()?;
    rows.next().is_none().then_some(first)
}
