//! Шаг 4 решения 7 (Тигран, 27.11.2026): у каждой строки DefiLlama Base и Ethereum вне списка
//! решения 7 — ровно один статус по факту: подключён проверенным профилем, нет спота (строка
//! исходника учёта), оборот < 5 000 USD или данных об обороте нет (снимок DefiLlama).
//! Спот с оборотом ≥ 5 000 USD без профиля не разрабатывается и вынесен владельцу.
//! Читает docs/BASE_ETHEREUM_SETTINGS.md, записанный снимок catalog_recheck.json и
//! config/first_run_dex.json; сверяет статусы и итоговую строку счётчиков каждой сети.
use serde_json::Value;
use std::collections::BTreeMap;

/// ID строк решения 7 по сети (включая отложенные им Dexalot, Clipper, Hashflow и Kaliber на Base).
/// Тот же список, что в tests/base_ethereum_catalog_coverage/decision_seven.rs.
const DECISION_SEVEN: [(&str, &str); 2] = [
    ("base", "5919 7984 8849 7945 8513 4725 5221 2803 8322 6675 2116 9146 7214 9153 534 4196 7626 7902 6049 8053 5181 2589 622 1447 7508"),
    ("ethereum", "7979 9436 2803 6650 2116 405 1447 7982 3104 8823 6049 7626 8767"),
];

/// Статус шага 4 по последней ячейке строки. Возвращает имя счётчика или None, если статуса нет.
fn status(decision: &str) -> Option<&'static str> {
    [
        ("подключён · профиль `", "profile"),
        ("нет спота · ", "nonspot"),
        ("оборот < 5 000 USD, отложено · ", "low"),
        ("спот · оборот < 5 000 USD, отложено", "low"),
        ("данных об обороте за сутки нет — отложено · ", "nodata"),
    ]
    .iter()
    .find(|(prefix, _)| decision.starts_with(prefix))
    .map(|(_, name)| *name)
    .or_else(|| decision.contains("вне решения 7 — вынесено владельцу").then_some("flag"))
}

#[test]
fn every_row_outside_decision_seven_has_one_factual_status() {
    let document = include_str!("../docs/BASE_ETHEREUM_SETTINGS.md");
    let snapshot: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/catalog_recheck.json"
    ))
    .unwrap();
    let profiles: Vec<Value> =
        serde_json::from_str(include_str!("../config/first_run_dex.json")).unwrap();
    for ((key, heading), (_, chosen)) in [("base", "### Base · DEX"), ("ethereum", "### Ethereum · DEX")]
        .into_iter()
        .zip(DECISION_SEVEN)
    {
        let chosen: Vec<&str> = chosen.split_whitespace().collect();
        let section = document.split_once(heading).unwrap().1;
        let table = section
            .split_once("#### DefiLlama")
            .unwrap()
            .1
            .split_once("| ID | Площадка | 24 ч, USD |")
            .unwrap()
            .1;
        let volumes = snapshot[key]["volume"]["by_id"].as_object().unwrap();
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        let mut total = 0;
        for row in table.lines().skip(2).take_while(|l| l.starts_with("| ")) {
            total += 1;
            let id = row.split('|').nth(1).unwrap().trim();
            let decision = row.rsplit('|').nth(1).unwrap().trim();
            if chosen.contains(&id) {
                *counts.entry("chosen").or_default() += 1;
                continue;
            }
            let volume = volumes.get(id).and_then(Value::as_f64);
            let name = status(decision).unwrap_or_else(|| panic!("{key} {id}: нет статуса шага 4"));
            match name {
                "profile" => {
                    let profile = decision["подключён · профиль `".len()..].split_once('`').unwrap().0;
                    assert!(
                        profiles.iter().any(|p| p["network"] == key && p["id"] == profile),
                        "{key} {id}: профиля {profile} нет в первом запуске"
                    );
                }
                "nonspot" => assert!(
                    decision.contains("https://") && decision.contains("#L"),
                    "{key} {id}: нет ссылки на строку исходника"
                ),
                "low" => assert!(volume.is_some_and(|v| v < 5_000.0), "{key} {id}: {volume:?}"),
                "nodata" => {
                    assert!(volume.is_none(), "{key} {id}: оборот есть в снимке");
                    assert!(decision.contains("(../tests/data/first_run_dex_2026-11-26/catalog_recheck.json)"));
                }
                _ => {
                    assert!(decision.starts_with("спот"), "{key} {id}");
                    assert!(volume.is_some_and(|v| v >= 5_000.0), "{key} {id}: {volume:?}");
                }
            }
            *counts.entry(name).or_default() += 1;
        }
        let get = |name| counts.get(name).copied().unwrap_or(0);
        assert_eq!(get("chosen"), chosen.len(), "{key}: строки решения 7");
        let summary = format!(
            "Из {total} строк {} относятся к решению 7 (включая отложенные им площадки) и в шаге 4 не менялись. \
             Остальные {}: подключён — {}, нет спота — {}, оборот < 5 000 USD — {}, данных об обороте нет — {}, вынесено владельцу — {}.",
            get("chosen"),
            total - get("chosen"),
            get("profile"),
            get("nonspot"),
            get("low"),
            get("nodata"),
            get("flag"),
        );
        assert!(section.contains(&summary), "{key}: {summary}");
    }
}
