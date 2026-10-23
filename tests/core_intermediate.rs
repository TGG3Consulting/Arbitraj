//! Файл: tests/core_intermediate.rs
//! Что делает: промежуточные монеты (решение 52) на настоящей SQLite во временной папке: список адресов BSC
//! проверяется по формату; выключено — только USDT; включено со списком — список; включено без списка или
//! переключатель не задан — честное «не задано». Плюс шаг маршрута у затраты (стык cost.rs).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::settings, arb::storage. Моков нет.

use arb::core::intermediate::{self, Intermediate};
use arb::core::types::{Cost, CostKind, RouteStep, VenueId};
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use rust_decimal::Decimal;
use serde_json::json;
use tokio::sync::broadcast;

/// WBNB в BSC — адрес из официальной документации BNB Chain (токен-обёртка BNB).
const WBNB: &str = "0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c";

/// Новая база во временной папке. Возвращает хранилище настроек.
async fn fresh(name: &str) -> Store {
    let dir = std::env::temp_dir().join(format!("arb-im-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("t.sqlite")).unwrap();
    let (tx, _) = broadcast::channel(16);
    let (jtx, _) = broadcast::channel(16);
    let journal = arb::storage::journal::Journal::new(db.clone(), jtx);
    Store::load(db, tx, journal).await.unwrap()
}

#[tokio::test]
async fn switch_and_list() {
    let s = fresh("switch").await;
    let dex = VenueId("bsc_dex".into());
    let sc = Scope::Venue(dex.0.clone());
    assert!(matches!(
        intermediate::read(&s, &dex),
        Intermediate::NotConfigured(_)
    ));
    s.set(
        "dex.intermediate_enabled",
        sc.clone(),
        Some(json!(false)),
        "тест",
    )
    .await
    .unwrap();
    assert_eq!(intermediate::read(&s, &dex), Intermediate::Off);
    s.set(
        "dex.intermediate_enabled",
        sc.clone(),
        Some(json!(true)),
        "тест",
    )
    .await
    .unwrap();
    assert!(matches!(
        intermediate::read(&s, &dex),
        Intermediate::NotConfigured(_)
    ));
    let bad = s
        .set(
            "dex.intermediate_tokens",
            sc.clone(),
            Some(json!(["BNB"])),
            "тест",
        )
        .await;
    assert!(bad.is_err(), "тикер вместо адреса должен отклоняться");
    s.set(
        "dex.intermediate_tokens",
        sc.clone(),
        Some(json!([])),
        "тест",
    )
    .await
    .unwrap();
    assert!(matches!(
        intermediate::read(&s, &dex),
        Intermediate::NotConfigured(_)
    ));
    s.set(
        "dex.intermediate_tokens",
        sc,
        Some(json!([WBNB, WBNB])),
        "тест",
    )
    .await
    .unwrap();
    assert_eq!(
        intermediate::read(&s, &dex),
        Intermediate::On(vec![WBNB.parse().unwrap()])
    );
}

#[test]
fn cost_carries_route_step() {
    let step = RouteStep {
        index: 1,
        from: "USDT".into(),
        to: "BNB".into(),
        from_token: None,
        to_token: Some(WBNB.parse().unwrap()),
    };
    let c = Cost::new(CostKind::PoolFee, Decimal::ONE, "USDT", true).with_step(step.clone());
    assert_eq!(c.step, Some(step));
    // Старый формат без шага разбирается (совместимость адаптеров и записей).
    let old: Cost =
        serde_json::from_value(json!({"kind":"Gas","amount":"1","asset":"BNB","in_price":false}))
            .unwrap();
    assert_eq!(old.step, None);
}
