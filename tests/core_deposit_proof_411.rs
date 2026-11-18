//! Файл: tests/core_deposit_proof_411.rs
//! Что делает: решение 411 (задача t196) — чистая логика сверки адреса депозита биржи-получателя
//! (core/deposit_proof_plan.rs) и памяти прогрева после смены ключей API (core/deposit_warm_plan.rs Memo::forget_all):
//! - compare: свежий ответ совпал с памятью / изменился / в памяти не было;
//! - for_withdraw: перевод берёт адрес только из свежего ответа сделки; свежего ответа нет (подтверждения нет, оно
//!   другого ключа или запрос упал) — свой свежий запрос, а не память;
//! - use_memory: предпроверка без ожидания — только если адрес в памяти и последний свежий запрос не падал;
//! - тексты журнала и Telegram — без значений адреса;
//! - Memo::forget_all: после смены секретов полученное забывается (прогрев проверит память под новым ключом).
//! Адреса — настоящие адреса BSC из записи tests/data/native_2026-10-25 (вывод BNB с BingX 24.10: отправитель и
//! получатель транзакции); к биржам тест не обращается.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::deposit_proof_plan, arb::core::deposit_warm_plan, arb::gateway::errors, serde_json.
//! Моков нет.

use arb::core::deposit_proof_plan::{
    Pick, Seen, State, changed_text, compare, failed_text, for_withdraw, use_memory,
};
use arb::core::deposit_warm_plan::Memo;
use serde_json::Value;

/// Два настоящих адреса BSC из записи: (отправитель, получатель) транзакции вывода BNB с BingX.
fn two_addresses() -> (String, String) {
    let p = "tests/data/native_2026-10-25/arb_bingx_bnb_withdraw_2026-10-25.json";
    let v: Value =
        serde_json::from_str(&std::fs::read_to_string(p).expect("запись")).expect("JSON");
    let tx = &v["results"][0]["response"]["result"];
    let from = tx["from"].as_str().expect("from").to_string();
    let to = tx["to"].as_str().expect("to").to_string();
    assert_ne!(from, to);
    (from, to)
}

#[test]
fn fresh_same_changed_first() {
    let (a, b) = two_addresses();
    assert_eq!(compare(Some(&a), &a), Seen::Same, "совпал");
    assert_eq!(compare(Some(&a), &b), Seen::Changed, "изменился");
    assert_eq!(compare(None, &b), Seen::First, "в памяти не было");
}

#[test]
fn withdraw_only_fresh_address() {
    let (a, _) = two_addresses();
    // Свежий ответ сделки пришёл — вывод на него (что бы ни лежало в памяти).
    assert_eq!(for_withdraw(State::Done(Ok(a.clone()))), Pick::Use(a));
    // Нет свежего ответа: подтверждения нет, оно не к этому переводу, свежий запрос упал — свой свежий запрос.
    for s in [
        State::Absent,
        State::Stale,
        State::Done(Err("нет адреса депозита".into())),
    ] {
        assert!(matches!(for_withdraw(s), Pick::Ask(_)));
    }
    // Адрес из памяти сам по себе в Pick::Use попасть не может: Use — только из State::Done(Ok).
}

#[test]
fn precheck_memory_only_when_confirmed() {
    assert!(
        use_memory(true, false),
        "в памяти и не было сбоя — без ожидания"
    );
    assert!(
        !use_memory(true, true),
        "последний свежий запрос упал — ждём ответ биржи"
    );
    assert!(
        !use_memory(false, false),
        "нет в памяти — ждём ответ биржи (как раньше)"
    );
}

#[test]
fn texts_without_address() {
    let (a, b) = two_addresses();
    let t = changed_text("kucoin", "ESIM", "BEP20");
    assert!(
        t.contains("kucoin") && t.contains("ESIM") && t.contains("изменился"),
        "{t}"
    );
    let f = failed_text("bitget", "LAB", "BEP20");
    for s in [&t, &f] {
        assert!(
            !s.contains(&a) && !s.contains(&b) && !s.contains("0x"),
            "{s}"
        );
    }
}

#[test]
fn warm_memo_forget_all() {
    let k = ("ESIM".to_string(), "BEP20".to_string());
    let mut m = Memo::default();
    let _ = m.record(k.clone(), Ok(()));
    assert!(!m.todo(&k), "получен — не запрашивается");
    m.forget_failed();
    assert!(!m.todo(&k), "forget_failed полученное не трогает");
    m.forget_all();
    assert!(m.todo(&k), "после смены секретов — проверить заново");
    assert_eq!(m.counts(), (0, 0));
}
