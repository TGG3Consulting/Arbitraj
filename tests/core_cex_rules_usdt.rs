//! Файл: tests/core_cex_rules_usdt.rs
//! Что делает: решение 249, дополнение 10.11 (Тигран: согласовано) — возврат выручки USDT на настоящем ядре и SQLite во
//! временной папке (площадок в реестре нет, моков логики нет; сделка — как живая DOGE bingx → mexc, выручка 83.10 USDT
//! на mexc, tests/usdt_return_common). Проверяется: (1) п. 1 — при закрытии возврат mexc → bingx запрещён списком
//! venue.cex_cex_deny mexc (биржи, через которую монета прошла), список bingx (биржи покупки) возврат на неё саму не
//! запрещает; (2) п. 4 — правило поменяли, пока возврат ждал повтора: попытки нет, «не вернулись» с причиной
//! «запрещено правилом: …» в хронологии; (3) п. 4 — «Перегнать» перепроверяет правило: запрещено — не отправляется,
//! снова «не вернулись» с той же причиной.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::gateway, arb::settings, arb::storage, arb::notify, arb::secrets,
//! tests/usdt_return_common.

mod usdt_return_common;

use std::time::Duration;

use arb::core::Core;
use arb::core::deal::Step;
use arb::core::types::{CommandSource, ManualAction, ManualCommand, VenueId};
use arb::core::usdt_return_state::UsdtReturn;
use arb::gateway::contract::VenueKind;
use arb::settings::value::Scope;
use rust_decimal::Decimal;
use serde_json::json;
use usdt_return_common::{dir, doge, start, wait_state};

/// Виды площадок ядра: bingx и mexc — биржи.
fn kinds(core: &Core) {
    *core.inner.kinds.write().unwrap() = vec![
        (VenueId("bingx".into()), VenueKind::Cex),
        (VenueId("mexc".into()), VenueKind::Cex),
    ];
}

/// Список venue.cex_cex_deny биржи `id` (None — очистить).
async fn deny(core: &Core, id: &str, list: Option<serde_json::Value>) {
    core.inner
        .deps
        .settings
        .set(
            arb::core::cex_link::DENY,
            Scope::Venue(id.into()),
            list,
            "тест",
        )
        .await
        .unwrap();
}

/// Строки хронологии сделки.
fn timeline(core: &Core, id: &str) -> Vec<String> {
    let d = core.deals().into_iter().find(|d| d.id == id).unwrap();
    d.timeline.into_iter().map(|(_, t)| t).collect()
}

const WHY: &str = "перегон USDT mexc → bingx запрещён: mexc не взаимодействует с bingx (список venue.cex_cex_deny у mexc)";

/// (1) Закрытие: список mexc запрещает возврат; список bingx — нет.
#[tokio::test]
async fn close_checks_list_of_path_exchange() {
    let (core, _) = start(&dir("c249-close"), None).await;
    kinds(&core);
    deny(&core, "mexc", Some(json!(["bingx"]))).await;
    let mut x = doge("c1DOGE");
    arb::core::ledger::finish(&core, &mut x, Step::Closed, "продано (выгодно)").await;
    let s = wait_state(&core, "c1DOGE", Duration::from_secs(5), |s| {
        matches!(s, UsdtReturn::NotReturned { .. })
    })
    .await;
    assert!(
        matches!(&s, Some(UsdtReturn::NotReturned { why, .. }) if why == WHY),
        "{s:?}"
    );
    // Список биржи покупки возврат на неё саму не запрещает: попытка идёт (здесь площадки нет — сбой перегона).
    deny(&core, "mexc", None).await;
    deny(&core, "bingx", Some(json!(["mexc"]))).await;
    let mut x = doge("c2DOGE");
    arb::core::ledger::finish(&core, &mut x, Step::Closed, "продано (выгодно)").await;
    let s = wait_state(&core, "c2DOGE", Duration::from_secs(5), |s| {
        matches!(s, UsdtReturn::NotReturned { .. })
    })
    .await;
    assert!(
        matches!(&s, Some(UsdtReturn::NotReturned { why, .. }) if why.starts_with("перегон не состоялся")),
        "{s:?}"
    );
}

/// (2) Повтор: правило поменяли между попытками — попытки нет, «не вернулись» с причиной «запрещено правилом».
#[tokio::test]
async fn retry_rechecks_rule() {
    let (core, _) = start(&dir("c249-retry"), Some((1, 3))).await;
    kinds(&core);
    let mut x = doge("c3DOGE");
    arb::core::ledger::finish(&core, &mut x, Step::Closed, "продано (выгодно)").await;
    // Первая попытка (площадки нет — сбой до вывода) — «ждёт повтора»; правило задано до повтора (через 1 с).
    deny(&core, "mexc", Some(json!(["bingx"]))).await;
    let s = wait_state(&core, "c3DOGE", Duration::from_secs(10), |s| {
        matches!(s, UsdtReturn::NotReturned { .. })
    })
    .await;
    let want = format!("запрещено правилом: {WHY}");
    assert!(
        matches!(&s, Some(UsdtReturn::NotReturned { why, .. }) if *why == want),
        "{s:?}"
    );
    let lines = timeline(&core, "c3DOGE");
    assert!(lines.iter().any(|t| t.contains(&want)), "{lines:?}");
    assert!(
        !lines
            .iter()
            .any(|t| t.starts_with("Возврат USDT: попытка 1")),
        "{lines:?}"
    );
}

/// (3) «Перегнать»: правило перепроверяется — запрещено, не отправляется, снова «не вернулись» с причиной.
#[tokio::test]
async fn manual_retry_rechecks_rule() {
    let (core, _) = start(&dir("c249-manual"), None).await;
    kinds(&core);
    let mut x = doge("c4DOGE");
    x.step = Step::Closed;
    x.usdt_return = Some(UsdtReturn::NotReturned {
        amount: Decimal::new(8310, 2),
        at: VenueId("mexc".into()),
        why: "перегон не состоялся".into(),
    });
    arb::core::ledger::save(&core, &x).await;
    deny(&core, "mexc", Some(json!(["bingx"]))).await;
    let t = core
        .command(ManualCommand {
            action: ManualAction::UsdtRetry {
                deal_id: "c4DOGE".into(),
            },
            source: CommandSource::Ui,
            by: Some("тест".into()),
            at: chrono::Utc::now(),
        })
        .await
        .unwrap();
    assert!(t.contains("запущен"), "{t}");
    let s = wait_state(
        &core,
        "c4DOGE",
        Duration::from_secs(5),
        |s| matches!(s, UsdtReturn::NotReturned { why, .. } if why.starts_with("«Перегнать»")),
    )
    .await;
    let want = format!("«Перегнать»: перегон не состоялся: запрещено правилом: {WHY}");
    assert!(
        matches!(&s, Some(UsdtReturn::NotReturned { why, .. }) if *why == want),
        "{s:?}"
    );
    assert!(timeline(&core, "c4DOGE").iter().any(|t| t.contains(&want)));
}
