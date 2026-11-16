//! Файл: tests/bsc_dex_pool_fail_339.rs
//! Что делает: решение 339 — расписание повтора пула после неудач чтения (pools/pool_fail.rs) и каталог настроек.
//! Проверяется: пауза удваивается с каждой неудачей подряд и упирается в потолок (потолок меньше начальной — начальная);
//! отметка пула: «пора читать» до первой неудачи, «нет» сразу после, «да» после паузы; журнал — только на первую
//! неудачу и на смену КОДА причины (ревью t153, Н4); снятие отметки; отметки разных пулов независимы (ошибка одного пула не трогает
//! другие). Ключи — адреса двух пулов из записанных квитанций BSC (tests/data/receipts_2026-10-25/responses.json).
//! Чего здесь нет (нужна запись с живого бота, docs/TASKS.md t153): ответ узла «execution reverted: KJUDGE_ERROR»
//! на eth_call пула DODO — сам путь сессии с ним не прогоняется.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::pools::pool_fail, arb::settings::{catalog, schema}, serde_json.
//! Настройки: нет.

use std::time::{Duration, Instant};

use alloy_primitives::Address;
use serde_json::Value;

use arb::core::types::PoolRef;
use arb::gateway::errors::ErrorCode;
use arb::settings::catalog;
use arb::settings::schema::{Kind, VenueFor};
use arb::venues::bsc_dex::pools::pool_fail::{Fails, pause_after};

const RECEIPTS: &str = "tests/data/receipts_2026-10-25/responses.json";

/// Два разных адреса контрактов из записанных логов (порядок узла).
fn two_pools() -> (PoolRef, PoolRef) {
    let text = std::fs::read_to_string(RECEIPTS).expect("запись квитанций");
    let v: Value = serde_json::from_str(&text).expect("JSON записи");
    let mut seen: Vec<Address> = Vec::new();
    for r in v["results"].as_array().expect("results") {
        let Some(list) = r["response"]["result"].as_array() else {
            continue;
        };
        for rc in list {
            for l in rc["logs"].as_array().into_iter().flatten() {
                let a: Address = l["address"]
                    .as_str()
                    .expect("address")
                    .parse()
                    .expect("адрес");
                if !seen.contains(&a) {
                    seen.push(a);
                }
                if seen.len() == 2 {
                    return (PoolRef::Contract(seen[0]), PoolRef::Contract(seen[1]));
                }
            }
        }
    }
    panic!("в записи меньше двух адресов");
}

#[test]
fn pause_doubles_up_to_cap() {
    assert_eq!(pause_after(500, 8_000, 0), 500, "0 неудач — как 1");
    assert_eq!(pause_after(500, 8_000, 1), 500);
    assert_eq!(pause_after(500, 8_000, 2), 1_000);
    assert_eq!(pause_after(500, 8_000, 5), 8_000);
    assert_eq!(pause_after(500, 8_000, 40), 8_000, "потолок");
    assert_eq!(
        pause_after(500, 100, 3),
        500,
        "потолок меньше начальной — начальная"
    );
    assert_eq!(
        pause_after(u64::MAX / 2, u64::MAX, 64),
        u64::MAX,
        "без переполнения"
    );
}

#[test]
fn one_pool_fail_does_not_touch_others() {
    let (a, b) = two_pools();
    let f = Fails::default();
    let t0 = Instant::now();
    // Откат eth_call узел отдаёт текстом «execution reverted…» — код SwapReverted (venues/bsc_dex/errors.rs from_rpc).
    let kj = (
        Some(ErrorCode::SwapReverted),
        "execution reverted: KJUDGE_ERROR",
    );
    assert!(f.due(&a, t0) && f.due(&b, t0), "без неудач — пора читать");
    let (x, fresh) = f.record(&a, kj, 1_000, 4_000, t0);
    assert!(fresh, "первая неудача — запись в журнал");
    assert_eq!((x.tries, x.pause_ms), (1, 1_000));
    assert!(!f.due(&a, t0), "пул на паузе");
    assert!(f.due(&b, t0), "другой пул пачки не затронут");
    assert!(f.due(&a, t0 + Duration::from_millis(1_000)), "пауза прошла");
    let (x, fresh) = f.record(&a, kj, 1_000, 4_000, t0);
    assert!(!fresh, "та же причина — журнал не повторяется");
    assert_eq!((x.tries, x.pause_ms), (2, 2_000));
    // Ревью t153 (Н4): причина сравнивается по коду ошибки — другой текст с тем же кодом журнал не повторяет.
    let (_, fresh) = f.record(
        &a,
        (
            Some(ErrorCode::SwapReverted),
            "execution reverted: другой текст",
        ),
        1_000,
        4_000,
        t0,
    );
    assert!(!fresh, "тот же код — журнал не повторяется");
    let (x, fresh) = f.record(
        &a,
        (Some(ErrorCode::BadResponse), "иная причина"),
        1_000,
        4_000,
        t0,
    );
    assert!(fresh, "смена кода причины — запись в журнал");
    assert_eq!((x.tries, x.pause_ms), (4, 4_000));
    let (x, _) = f.record(
        &a,
        (Some(ErrorCode::BadResponse), "иная причина"),
        1_000,
        4_000,
        t0,
    );
    assert_eq!(x.pause_ms, 4_000, "не больше потолка");
    assert!(f.get(&b).is_none());
    f.clear(&a);
    assert!(
        f.get(&a).is_none() && f.due(&a, t0),
        "прочитан — отметки нет"
    );
}

#[test]
fn catalog_has_339_settings() {
    for name in ["dex.pool_fail_pause_ms", "dex.pool_fail_pause_max_ms"] {
        let def = catalog::find(name).unwrap_or_else(|| panic!("{name} в каталоге"));
        assert!(def.optional, "{name}: необязательная");
        assert_eq!(def.kind, Kind::Integer, "{name}: целое");
        assert!(
            matches!(def.venue_for, VenueFor::Dex),
            "{name}: только площадка DEX"
        );
    }
}
