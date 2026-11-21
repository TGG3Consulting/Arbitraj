//! Файл: tests/core_status_poll_398.rs
//! Что делает: решение 398 (ревью t183, п. 1) — venue.status_poll_ms проверяется до покупки сделки с переводом.
//! (1) core::transfers::poll_missing на настоящем ядре (SQLite во временной папке, tests/usdt_return_common): ключ не
//!     задан ни у кого — причина «не задана venue.status_poll_ms у <отправитель>»; задан только у отправителя — причина
//!     у получателя; задан у обоих — проверка пройдена; ключ у получателя нужен и тогда, когда получатель — DEX
//!     (шапка poll_missing: зачисление опросом и подтверждение статусом).
//! (2) Страж по исходникам: исполнитель сделки (core/executor.rs run) вызывает poll_missing после предпроверки и до
//!     покупки (первого trade_ops::trade); вывод (core/transfers.rs go) — до gw_from.withdraw (вторая линия).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core (transfers), arb::settings, tests/usdt_return_common. Моков нет.

mod usdt_return_common;

use arb::core::transfers::poll_missing;
use arb::core::types::VenueId;
use arb::settings::value::Scope;
use serde_json::json;
use usdt_return_common::{dir, start_with};

#[tokio::test]
async fn status_poll_required_on_both_sides() {
    let (bingx, mexc, dex) = (
        VenueId("bingx".into()),
        VenueId("mexc".into()),
        VenueId("bsc_dex".into()),
    );
    let d = dir("398-none");
    let (core, _j) = start_with(&d, None, Vec::new()).await;
    let why = poll_missing(&core, &bingx, &mexc).expect("не задана нигде");
    assert!(
        why.contains("не задана venue.status_poll_ms у bingx"),
        "{why}"
    );

    let d = dir("398-from");
    let set = vec![(
        "venue.status_poll_ms",
        Scope::Venue("bingx".into()),
        json!(500),
    )];
    let (core, _j) = start_with(&d, None, set).await;
    let why = poll_missing(&core, &bingx, &mexc).expect("у получателя не задана");
    assert!(
        why.contains("не задана venue.status_poll_ms у mexc"),
        "{why}"
    );
    let why = poll_missing(&core, &bingx, &dex).expect("у DEX-получателя не задана");
    assert!(why.contains("у bsc_dex"), "{why}");

    let d = dir("398-both");
    let set = vec![
        (
            "venue.status_poll_ms",
            Scope::Venue("bingx".into()),
            json!(500),
        ),
        (
            "venue.status_poll_ms",
            Scope::Venue("mexc".into()),
            json!(500),
        ),
    ];
    let (core, _j) = start_with(&d, None, set).await;
    assert_eq!(poll_missing(&core, &bingx, &mexc), None);
}

/// Строки файла от корня. Паника — файла нет.
fn read(rel: &str) -> String {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// Номер первой строки с подстрокой. Паника — нет.
fn line_of(src: &str, part: &str, file: &str) -> usize {
    src.lines()
        .position(|l| l.contains(part))
        .unwrap_or_else(|| panic!("{file}: нет «{part}»"))
}

#[test]
fn checked_before_buy_and_before_withdraw() {
    let f = "src/core/executor.rs";
    let s = read(f);
    let check = line_of(
        &s,
        "transfers::poll_missing(core, &d.buy_on, &d.sell_on)",
        f,
    );
    assert!(
        check > line_of(&s, "precheck::run(", f),
        "после предпроверки"
    );
    assert!(check < line_of(&s, "trade_ops::trade(", f), "до покупки");
    let f = "src/core/transfers.rs";
    let s = read(f);
    assert!(
        line_of(&s, "poll_set(core, deal, from, to, asset).await?", f)
            < line_of(&s, "gw_from.withdraw(", f),
        "вторая линия — до вывода"
    );
}
