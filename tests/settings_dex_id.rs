//! Файл: tests/settings_dex_id.rs
//! Что делает: решение 128 — правило записи id области «DEX» (строчная латиница, цифры, «-», «_», непустой) и предел
//! длины нового id (dex.id_max_len). Проверка на реальных id: все id DEX сети bsc из записи GeckoTerminal 2026-10-24
//! (tests/data/indexers_2026-10-24) проходят правило — значит, id из справочника пулов всегда можно сохранить.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::settings::scope, arb::venues::bsc_dex::directory::gt_parse.
//! Настройки: нет.

use arb::settings::scope::{check_dex_id, check_new_dex_id_len};
use arb::venues::bsc_dex::directory::gt_parse;
use serde_json::Value;

/// Тело записи по имени. Принимает: имя записи. Возвращает: тело ответа.
fn body(name: &str) -> String {
    let text =
        std::fs::read_to_string("tests/data/indexers_2026-10-24/arb_indexers_2026-10-24.json")
            .expect("файл записи");
    let all: Value = serde_json::from_str(&text).expect("JSON");
    all[name]["body"].as_str().expect("body").to_string()
}

#[test]
fn dex_id_rule() {
    for ok in [
        "maverick-v2-bsc",
        "pancakeswap_v2",
        "biswap-v3-1",
        "lista-stable",
        "a",
    ] {
        assert!(check_dex_id(ok).is_ok(), "{ok}");
    }
    for bad in [
        "",
        "Maverick",
        "pancake swap",
        "dex.v2",
        "dex/1",
        "листа",
        " x",
    ] {
        assert!(check_dex_id(bad).is_err(), "«{bad}» должен быть отклонён");
    }
}

#[test]
fn recorded_indexer_ids_pass_rule() {
    let (l1, _) = gt_parse::dexes(&body("gt_dexes_bsc_p1")).unwrap();
    let (l2, _) = gt_parse::dexes(&body("gt_dexes_bsc_p2")).unwrap();
    assert_eq!(l1.len() + l2.len(), 139);
    for d in l1.iter().chain(&l2) {
        assert!(check_dex_id(&d.id).is_ok(), "id индексатора {}", d.id);
    }
}

#[test]
fn new_id_length_limit() {
    // Не задана — без предела.
    assert!(check_new_dex_id_len("a-very-long-dex-id-without-limit", None, false).is_ok());
    // Новый id длиннее — отказ, не длиннее — да.
    assert!(check_new_dex_id_len("maverick-v2-bsc", Some(10), false).is_err());
    assert!(check_new_dex_id_len("maverick-v2-bsc", Some(15), false).is_ok());
    // Уже существующий id не проверяется (снижение предела не запирает его настройки).
    assert!(check_new_dex_id_len("maverick-v2-bsc", Some(3), true).is_ok());
}
