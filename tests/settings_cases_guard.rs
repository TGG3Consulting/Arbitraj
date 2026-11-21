//! Файл: tests/settings_cases_guard.rs
//! Что делает: сторож решения 396 (Тигран 17.11.2026): у каждой настройки каталога есть запись в журнале
//! docs/SETTINGS_CASES.md (кейс, что ставить, поведение если не задана) — или ключ в списке долга
//! tests/data/settings_cases_legacy.txt (старые ключи, появившиеся до t165). Источник истины ключей — код:
//! arb::settings::catalog::all() (реестр всех частей src/settings/catalog/*.rs). Проверки:
//! - ключ каталога без записи и не в долге — ошибка «добавь запись в docs/SETTINGS_CASES.md»;
//! - у записи пустое или отсутствующее поле «Кейс», «Что ставить» или «Если не задана» — ошибка;
//! - запись дважды — ошибка; запись о ключе, которого нет в каталоге, — только со статусом «ещё не в релизе» или «удалена»;
//! - ключ в долге, которого нет в каталоге, ключ в долге и в журнале сразу, повтор в долге — ошибка (долг только
//!   уменьшается).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::settings::catalog, файлы docs/SETTINGS_CASES.md и tests/data/settings_cases_legacy.txt. Моков нет.

use std::collections::{BTreeMap, BTreeSet};

use arb::settings::catalog;

/// Поля, которые обязаны быть непустыми в каждой записи журнала.
const REQUIRED: [&str; 3] = ["Кейс", "Что ставить", "Если не задана"];

/// Что делает: читает файл из корня проекта. Принимает: путь от корня. Возвращает: текст. Ошибки: паника с причиной.
fn read(rel: &str) -> String {
    let p = format!("{}/{rel}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("не читается {p}: {e}"))
}

/// Что делает: разбирает журнал на записи. Принимает: текст docs/SETTINGS_CASES.md. Возвращает: ключ → (поле → значение);
/// заголовок записи — строка «### `ключ`», поле — строка «- **Имя:** значение». Повтор записи — в списке дублей.
fn records(text: &str) -> (BTreeMap<String, BTreeMap<String, String>>, Vec<String>) {
    let mut out: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let mut dups = vec![];
    let mut cur: Option<String> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("### `")
            && let Some(key) = rest.strip_suffix('`')
        {
            if out.insert(key.to_string(), BTreeMap::new()).is_some() {
                dups.push(key.to_string());
            }
            cur = Some(key.to_string());
            continue;
        }
        if line.starts_with("## ") || line.starts_with("### ") {
            cur = None;
            continue;
        }
        if let Some(key) = &cur
            && let Some(rest) = line.strip_prefix("- **")
            && let Some((name, value)) = rest.split_once(":**")
            && let Some(fields) = out.get_mut(key)
        {
            fields.insert(name.trim().to_string(), value.trim().to_string());
        }
    }
    (out, dups)
}

/// Что делает: ключи списка долга. Принимает: текст файла. Возвращает: ключи по порядку (пустые и «#» пропущены).
fn legacy(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect()
}

#[test]
fn every_setting_has_case_record() {
    let keys: BTreeSet<String> = catalog::all().map(|s| s.name.to_string()).collect();
    let (recs, dups) = records(&read("docs/SETTINGS_CASES.md"));
    let debt_list = legacy(&read("tests/data/settings_cases_legacy.txt"));
    let mut errors: Vec<String> = dups
        .iter()
        .map(|k| format!("{k}: запись в docs/SETTINGS_CASES.md повторяется — оставь одну"))
        .collect();

    let mut debt = BTreeSet::new();
    for k in &debt_list {
        if !debt.insert(k.clone()) {
            errors.push(format!(
                "{k}: повтор в tests/data/settings_cases_legacy.txt"
            ));
        }
        if !keys.contains(k) {
            errors.push(format!(
                "{k}: ключа нет в каталоге настроек — убери его из tests/data/settings_cases_legacy.txt (долг только уменьшается)"
            ));
        }
        if recs.contains_key(k) {
            errors.push(format!(
                "{k}: описан в docs/SETTINGS_CASES.md — убери его из tests/data/settings_cases_legacy.txt"
            ));
        }
    }

    for k in &keys {
        match recs.get(k) {
            None if !debt.contains(k) => errors.push(format!(
                "{k}: новая настройка без записи — добавь запись в docs/SETTINGS_CASES.md (кейс, что ставить, \
                 поведение если не задана; правило Тиграна 17.11.2026, решение 396)"
            )),
            None => {}
            Some(fields) => {
                for f in REQUIRED {
                    if fields.get(f).is_none_or(|v| v.is_empty()) {
                        errors.push(format!(
                            "{k}: в записи docs/SETTINGS_CASES.md пустое или нет поля «{f}» — заполни его"
                        ));
                    }
                }
            }
        }
    }

    for (k, fields) in &recs {
        if keys.contains(k) {
            continue;
        }
        let status = fields.get("Статус").map(String::as_str).unwrap_or("");
        if !(status.contains("ещё не в релизе") || status.contains("удалена")) {
            errors.push(format!(
                "{k}: записи в docs/SETTINGS_CASES.md нет ключа в каталоге — проверь имя; для ключа из неслитой ветки \
                 статус «ещё не в релизе (ветка …)», для удалённого — «удалена»"
            ));
        }
    }

    assert!(
        errors.is_empty(),
        "Журнал настроек docs/SETTINGS_CASES.md не сходится с каталогом ({} ошибок):\n{}",
        errors.len(),
        errors.join("\n")
    );
}
