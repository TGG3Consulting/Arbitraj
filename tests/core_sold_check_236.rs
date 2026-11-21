//! Файл: tests/core_sold_check_236.rs
//! Что делает: решение 236, дополнение п. 3 (техлид 09.11) — проверка ввода «Продано вручную» по стоимости остатка на
//! рынке (manual.sold_max_over_pct, core/manual_sold_check.rs). Ядро на настоящей SQLite во временной папке без площадок
//! в реестре (моков нет), сделка — как живая 20261104225504576DOGE, остановленная с остатком.
//! Проверяется: (1) настройка не задана — проверки нет, как раньше (пометки нет); (2) настройка задана, котировки нет
//! (площадки нет в реестре) — проверки нет, сделка закрывается, пометка «не выполнена — нет котировки» в ответе и
//! хронологии; (3) остатка нет — проверять нечего. Отказ при вводе больше стоимости по рынку требует живой котировки
//! биржи — в тесте без площадок не проверяется (арифметика границы — value × (1 + N/100), manual_sold_check.rs).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::settings, arb::storage, arb::notify, arb::secrets.

mod usdt_return_common;

use arb::core::Core;
use arb::core::deal::{Deal, Step};
use arb::core::types::{CommandSource, ManualAction, ManualCommand};
use arb::settings::value::Scope;
use rust_decimal::Decimal;
use serde_json::json;
use usdt_return_common::{dir, doge, start, start_with};

/// Остановленная DOGE с остатком qty на mexc (вложено 80, выручки бота нет). Ошибок нет.
async fn halted(core: &Core, id: &str, qty: i64) {
    let mut x = doge(id);
    x.proceeds = Decimal::ZERO;
    x.qty = Decimal::from(qty);
    x.step = Step::Halted;
    arb::core::ledger::save(core, &x).await;
}

/// «Продано вручную» из интерфейса.
async fn sold(core: &Core, id: &str, usdt: &str) -> Result<String, String> {
    core.command(ManualCommand {
        action: ManualAction::SoldManually {
            deal_id: id.into(),
            usdt: usdt.into(),
        },
        source: CommandSource::Ui,
        by: Some("тест".into()),
        at: chrono::Utc::now(),
    })
    .await
}

/// Сделка из учёта ядра.
fn deal(core: &Core, id: &str) -> Deal {
    core.deals().into_iter().find(|x| x.id == id).unwrap()
}

#[tokio::test]
async fn not_set_means_no_check() {
    let (core, _) = start(&dir("s236n"), None).await;
    halted(&core, "k1DOGE", 1000).await;
    let t = sold(&core, "k1DOGE", "83.10").await.unwrap();
    assert!(!t.contains("проверка ввода"), "{t}");
    assert_eq!(deal(&core, "k1DOGE").step, Step::Closed);
}

#[tokio::test]
async fn no_quote_means_no_check_with_note() {
    let set = vec![("manual.sold_max_over_pct", Scope::Global, json!("20"))];
    let (core, _) = start_with(&dir("s236q"), None, set).await;
    halted(&core, "k2DOGE", 1000).await;
    let t = sold(&core, "k2DOGE", "83.10").await.unwrap();
    assert!(
        t.contains("проверка ввода (manual.sold_max_over_pct) не выполнена — нет котировки"),
        "{t}"
    );
    let y = deal(&core, "k2DOGE");
    assert_eq!(y.step, Step::Closed);
    assert!(
        y.timeline
            .iter()
            .any(|(_, t)| t.contains("Продано вручную: проверка ввода")),
        "{:?}",
        y.timeline
    );
    // Остатка нет — проверять нечего (пометки нет).
    halted(&core, "k3DOGE", 0).await;
    let t = sold(&core, "k3DOGE", "1").await.unwrap();
    assert!(!t.contains("проверка ввода"), "{t}");
}
