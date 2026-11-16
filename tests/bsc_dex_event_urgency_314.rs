//! Файл: tests/bsc_dex_event_urgency_314.rs
//! Что делает: решение 314 — класс очереди узла для перечитываний пула по событию (pools/refresh_urgency.rs), чистая
//! логика без данных площадок:
//! - каждый вариант настройки dex.event_refresh_urgency из каталога переводится в свой класс gateway/urgency.rs, иное
//!   значение — без класса (как не задана);
//! - класс действует только внутри перечитывания и не утекает на остальную работу задачи (сессии пачки): до и после —
//!   класс задачи прежний; без настройки (None) класс внутри — прежний.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::settings::catalog, arb::gateway::urgency, arb::venues::bsc_dex::pools::refresh_urgency.
//! Настройки: нет.

use arb::gateway::urgency::{self, Urgency};
use arb::settings::catalog;
use arb::venues::bsc_dex::pools::refresh_urgency::{parse, with};

#[test]
fn every_catalog_choice_is_a_class_and_nothing_else() {
    let def = catalog::find("dex.event_refresh_urgency").expect("настройка в каталоге");
    assert!(def.optional, "настройка необязательная");
    let classes: Vec<Urgency> = def
        .choices
        .iter()
        .map(|c| parse(c).unwrap_or_else(|| panic!("вариант {c} без класса")))
        .collect();
    assert_eq!(
        classes,
        vec![
            Urgency::Trade,
            Urgency::Check,
            Urgency::Quote,
            Urgency::Background
        ]
    );
    assert_eq!(parse(""), None);
    assert_eq!(parse("Trade"), None);
}

#[tokio::test]
async fn class_lives_only_inside_the_refresh() {
    // Сессия пачки без класса: до, внутри (Trade) и после.
    assert_eq!(urgency::is_set(), None);
    let inside = with(Some(Urgency::Trade), async { urgency::current() }).await;
    assert_eq!(inside, Urgency::Trade);
    assert_eq!(urgency::is_set(), None);
    assert_eq!(urgency::current(), Urgency::Background);
    // Не задана — класс внутри прежний.
    let same = with(None, async { urgency::is_set() }).await;
    assert_eq!(same, None);
    // Задача с классом: после перечитывания класс задачи тот же.
    urgency::scope(Urgency::Check, async {
        let inner = with(Some(Urgency::Quote), async { urgency::current() }).await;
        assert_eq!(inner, Urgency::Quote);
        assert_eq!(urgency::current(), Urgency::Check);
    })
    .await;
}
