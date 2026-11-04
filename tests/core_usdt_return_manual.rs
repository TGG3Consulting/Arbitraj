//! Файл: tests/core_usdt_return_manual.rs
//! Что делает: решение 163 — запись «USDT не вернулись» на экране «Ручное управление» (та же уходит в Telegram,
//! решение 156) на настоящей SQLite без площадок в реестре (моков нет). Сделка — как живая 20261104225504576DOGE
//! (bingx → mexc, 83.10 USDT остались на MEXC после отказа 16021, состояние not_returned): (1) такая сделка из учёта
//! прошлого запуска сразу на экране с кнопками «Перегнать» и «Перегнано вручную» (кнопок «Продать» нет);
//! (2) «Перегнано вручную» закрывает запись — возврат «вручную», кто нажал — в журнале и хронологии; повторное
//! нажатие — без повторного действия; (3) «Перегнать» — одна попытка той же командой ядра, здесь площадки нет —
//! снова «не вернулись» с причиной, запись возвращается; (4) «Продано вручную» (решение 100) — «Перегнать» недоступна.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::settings, arb::storage, arb::notify, arb::secrets.

mod usdt_return_common;

use std::time::Duration;

use arb::core::Core;
use arb::core::deal::Step;
use arb::core::types::{CommandSource, ManualAction, ManualCommand, VenueId};
use arb::core::usdt_return_state::UsdtReturn;
use rust_decimal::Decimal;
use usdt_return_common::{dir, doge, entries, start, wait_state};

/// Закрытая сделка DOGE с «не вернулись» (как в учёте живого бота после отказа MEXC 16021). Ошибок нет.
async fn not_returned(core: &Core, id: &str, skip: Option<&str>) {
    let mut x = doge(id);
    x.step = Step::Closed;
    x.usdt_return_skip = skip.map(str::to_string);
    x.usdt_return = Some(UsdtReturn::NotReturned {
        amount: Decimal::new(8310, 2),
        at: VenueId("mexc".into()),
        why: "перегон не состоялся: MEXC, код 16021: Some pre-crediting assets are temporarily unavailable for withdrawal. Please reconfirm the quantity.".into(),
    });
    arb::core::ledger::save(core, &x).await;
}

/// Нажатие кнопки записи: команда ядру из интерфейса (тот же путь Core::command, что у кнопки Telegram).
/// Возвращает итог ядра.
async fn press(core: &Core, action: ManualAction) -> Result<String, String> {
    core.command(ManualCommand {
        action,
        source: CommandSource::Ui,
        by: Some("тест".into()),
        at: chrono::Utc::now(),
    })
    .await
}

#[tokio::test]
async fn old_not_returned_is_on_screen_after_restart() {
    let d = dir("old");
    {
        let (core, _) = start(&d, Some((10, 5))).await;
        not_returned(&core, "m1DOGE", None).await;
    }
    let (core, _) = start(&d, Some((10, 5))).await;
    let rec = core
        .manual_records()
        .into_iter()
        .find(|r| r.key == "u.m1DOGE")
        .unwrap();
    let titles: Vec<&str> = rec.buttons.iter().map(|b| b.title.as_str()).collect();
    assert_eq!(titles, ["Перегнать", "Перегнано вручную"]);
    assert!(
        rec.buttons
            .iter()
            .all(|b| b.disabled.is_none() && b.input.is_none())
    );
    assert!(
        rec.text
            .contains("Причина: перегон не состоялся: MEXC, код 16021")
    );
    // Таблица экрана — поле usdt в GET /api/manual.
    assert_eq!(core.manual()["usdt"][0]["id"], "m1DOGE");
}

#[tokio::test]
async fn returned_manually_closes_and_second_press_does_nothing() {
    let (core, j) = start(&dir("man"), None).await;
    not_returned(&core, "m2DOGE", None).await;
    let a = ManualAction::UsdtReturnedManually {
        deal_id: "m2DOGE".into(),
    };
    let t = press(&core, a.clone()).await.unwrap();
    assert!(t.contains("перегнано вручную"), "{t}");
    let d = core.deals().into_iter().find(|d| d.id == "m2DOGE").unwrap();
    assert!(matches!(&d.usdt_return, Some(UsdtReturn::Manual { by, .. }) if by == "интерфейс"));
    assert!(core.manual_records().iter().all(|r| r.key != "u.m2DOGE"));
    assert_eq!(
        entries(&j, "core.usdt_return", "перегнано вручную")
            .await
            .len(),
        1
    );
    let lines = d.timeline.len();
    // Повторное нажатие — «проигнорирована», состояние и хронология те же.
    let again = press(&core, a).await.unwrap();
    assert!(again.contains("проигнорирована"), "{again}");
    let d2 = core.deals().into_iter().find(|d| d.id == "m2DOGE").unwrap();
    assert_eq!(d2.timeline.len(), lines);
    assert_eq!(d2.usdt_return, d.usdt_return);
}

#[tokio::test]
async fn retry_button_makes_one_attempt() {
    let (core, j) = start(&dir("again"), Some((10, 5))).await;
    not_returned(&core, "m3DOGE", None).await;
    let a = ManualAction::UsdtRetry {
        deal_id: "m3DOGE".into(),
    };
    let t = press(&core, a).await.unwrap();
    assert!(t.contains("перегон USDT mexc → bingx запущен"), "{t}");
    // Площадки mexc нет — попытка не состоялась: снова «не вернулись» (без автоповторов), запись вернулась.
    let s = wait_state(
        &core,
        "m3DOGE",
        Duration::from_secs(5),
        |s| matches!(s, UsdtReturn::NotReturned { why, .. } if why.starts_with("«Перегнать»")),
    )
    .await;
    assert!(matches!(s, Some(UsdtReturn::NotReturned { .. })), "{s:?}");
    assert!(core.manual_records().iter().any(|r| r.key == "u.m3DOGE"));
    assert_eq!(
        entries(&j, "core.usdt_not_returned", "m3DOGE").await.len(),
        1
    );
}

#[tokio::test]
async fn sold_manually_has_no_retry() {
    let (core, _) = start(&dir("skip"), None).await;
    not_returned(
        &core,
        "m4DOGE",
        Some("продано вручную человеком — бот выручку не перегоняет (решение 100)"),
    )
    .await;
    let rec = core
        .manual_records()
        .into_iter()
        .find(|r| r.key == "u.m4DOGE")
        .unwrap();
    assert!(rec.button("usdt_retry").unwrap().disabled.is_some());
    assert!(rec.button("usdt_manual").unwrap().disabled.is_none());
    let e = press(
        &core,
        ManualAction::UsdtRetry {
            deal_id: "m4DOGE".into(),
        },
    )
    .await;
    assert!(e.is_err());
}
