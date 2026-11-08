//! Файл: tests/core_usdt_return_retry.rs
//! Что делает: решение 163 на настоящей SQLite во временной папке (ядро без площадок в реестре, как остальные тесты
//! учёта; моков нет). Живой случай 04.11: сделка 20261104225504576DOGE bingx → mexc закрыта с прибылью, возврат выручки
//! 83.10 USDT mexc → bingx отказан MEXC 16021 (ответ разбирает настоящий адаптер — venues/mexc/errors.rs). Проверяется:
//! (1) какие сбои безопасно повторять (16021 — да; исход неизвестен, сервер занят — нет); (2) повтор N раз с интервалом
//! usdt.return_retry_s, строка «Возврат USDT: попытка N из M — причина» на каждую, затем запись «USDT не вернулись» с
//! двумя кнопками и одной страховкой; (3) успех на k-й попытке — записи нет; (4) без настроек — без повторов, журнал
//! один раз; (5) «ждёт повтора» переживает перезапуск. Кнопки записи — tests/core_usdt_return_manual.rs.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::settings, arb::storage, arb::notify, arb::secrets, arb::venues::mexc::errors.

mod usdt_return_common;

use std::time::{Duration, Instant};

use arb::core::deal::Step;
use arb::core::op_error::OpError;
use arb::core::types::VenueId;
use arb::core::usdt_return_retry::{fail, retry_safe};
use arb::core::usdt_return_state::{self as st, UsdtReturn};
use arb::gateway::errors::{ErrorCode, GatewayError};
use arb::venues::mexc::errors::{Op, from_code};
use rust_decimal::Decimal;
use usdt_return_common::{dir, doge, entries, start, wait_state};

/// Ответ MEXC 16021 (текст — из журнала живого случая; разбор — настоящий адаптер).
fn mexc_16021() -> OpError {
    let msg = "Some pre-crediting assets are temporarily unavailable for withdrawal. Please reconfirm the quantity.";
    let raw = format!(r#"{{"code":16021,"msg":"{msg}"}}"#);
    OpError::from(&from_code(16021, msg, Op::Withdraw, &raw))
}

#[test]
fn which_failures_are_safe_to_retry() {
    let e = mexc_16021();
    // Решение 166: 16021 — единый код «временно заблокировано для вывода».
    assert_eq!(e.code, Some(ErrorCode::WithdrawLocked));
    assert!(retry_safe(&e), "16021 — вывод отказан, денег не ушло");
    // Ошибка до вывода (своя, без кода) — повтор безопасен.
    assert!(retry_safe(&OpError::from(
        "mexc: свободные USDT: площадка не подключена".to_string()
    )));
    // Сервер MEXC занят на выводе — итог неизвестен (Timeout), вывод мог создаться: повтора нет.
    let busy = OpError::from(&from_code(503, "busy", Op::Withdraw, "{}"));
    assert_eq!(busy.code, Some(ErrorCode::Timeout));
    assert!(!retry_safe(&busy));
    let unknown = GatewayError::new(ErrorCode::OutcomeUnknown, None, "вывод не найден в истории");
    assert!(!retry_safe(&OpError::from(&unknown)));
}

#[tokio::test]
async fn retries_with_interval_then_record_with_two_buttons() {
    let (core, j) = start(&dir("retry"), Some((1, 2))).await;
    let mut x = doge("r1DOGE");
    let t0 = Instant::now();
    arb::core::ledger::finish(&core, &mut x, Step::Closed, "продано (выгодно)").await;
    // Площадки mexc в реестре нет: каждая попытка — сбой до вывода (повтор безопасен), 2 повтора по 1 с.
    let s = wait_state(&core, "r1DOGE", Duration::from_secs(10), |s| {
        matches!(s, UsdtReturn::NotReturned { .. })
    })
    .await;
    assert!(
        t0.elapsed() >= Duration::from_secs(2),
        "два повтора с интервалом 1 с"
    );
    match s {
        Some(UsdtReturn::NotReturned { amount, at, why }) => {
            assert_eq!(amount, Decimal::new(8310, 2));
            assert_eq!(at, VenueId("mexc".into()));
            assert!(why.contains("все 2 повторов не удались"), "{why}");
        }
        other => panic!("ждали «не вернулись», а {other:?}"),
    }
    let d = core.deals().into_iter().find(|d| d.id == "r1DOGE").unwrap();
    let lines: Vec<&String> = d.timeline.iter().map(|(_, t)| t).collect();
    assert!(
        lines
            .iter()
            .any(|t| t.starts_with("Возврат USDT не состоялся"))
    );
    assert!(
        lines
            .iter()
            .any(|t| t.starts_with("Возврат USDT: попытка 1 из 2 — "))
    );
    assert!(
        lines
            .iter()
            .any(|t| t.starts_with("Возврат USDT: попытка 2 из 2 — "))
    );
    assert_eq!(
        entries(&j, "core.usdt_return", "попытка 2 из 2")
            .await
            .len(),
        1
    );
    // Страховка — одна, после всех повторов; запись экрана — с двумя кнопками, без «Продать».
    assert_eq!(
        entries(&j, "core.usdt_not_returned", "r1DOGE").await.len(),
        1
    );
    let rec = core
        .manual_records()
        .into_iter()
        .find(|r| r.key == "u.r1DOGE")
        .unwrap();
    let codes: Vec<&str> = rec.buttons.iter().map(|b| b.code.as_str()).collect();
    assert_eq!(codes, ["usdt_retry", "usdt_manual"]);
    assert!(
        rec.text.contains("83.10 USDT") && rec.text.contains("MEXC") && rec.text.contains("BingX"),
        "{}",
        rec.text
    );
}

#[tokio::test]
async fn success_on_kth_attempt_leaves_no_record() {
    let (core, j) = start(&dir("ok"), Some((10, 5))).await;
    let mut x = doge("r2DOGE");
    x.step = Step::Closed;
    arb::core::ledger::save(&core, &x).await;
    let (amount, from, to) = (
        Decimal::new(8310, 2),
        VenueId("mexc".into()),
        VenueId("bingx".into()),
    );
    let e = mexc_16021();
    // Сбой первой попытки и двух повторов (16021) — каждый раз «ждёт повтора».
    for tries in 0..3 {
        assert!(fail(&core, "r2DOGE", amount, (&from, &to), tries, &e).await);
    }
    let d = core.deals().into_iter().find(|d| d.id == "r2DOGE").unwrap();
    assert!(matches!(
        d.usdt_return,
        Some(UsdtReturn::Retrying { tries: 2, .. })
    ));
    assert!(
        d.timeline
            .iter()
            .any(|(_, t)| t.contains("попытка 2 из 5 — MEXC, код 16021"))
    );
    // Третий повтор прошёл (итог, который пишет задача повторов) — записи на экране нет, страховки нет.
    let done = UsdtReturn::Done {
        amount,
        credited: amount,
        from,
        to,
        sent: None,
        fee_usd: None,
    };
    st::set(&core, "r2DOGE", done, "USDT вернулись на попытке 3 из 5").await;
    assert!(core.manual_records().iter().all(|r| r.key != "u.r2DOGE"));
    assert!(
        entries(&j, "core.usdt_not_returned", "r2DOGE")
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn without_settings_no_retry_and_one_journal_line() {
    let (core, j) = start(&dir("noset"), None).await;
    let mut x = doge("r3DOGE");
    arb::core::ledger::finish(&core, &mut x, Step::Closed, "продано (выгодно)").await;
    let s = wait_state(&core, "r3DOGE", Duration::from_secs(5), |s| {
        matches!(s, UsdtReturn::NotReturned { .. })
    })
    .await;
    assert!(
        matches!(s, Some(UsdtReturn::NotReturned { why, .. }) if why.contains("перегон не состоялся"))
    );
    assert_eq!(
        entries(&j, "core.usdt_return", "повторы возврата USDT не заданы")
            .await
            .len(),
        1
    );
    assert!(core.manual_records().iter().any(|r| r.key == "u.r3DOGE"));
}

#[tokio::test]
async fn retrying_survives_restart() {
    let d = dir("restart");
    {
        let (core, _) = start(&d, Some((1, 1))).await;
        let mut x = doge("r4DOGE");
        x.step = Step::Closed;
        x.usdt_return = Some(UsdtReturn::Retrying {
            amount: Decimal::new(8310, 2),
            from: VenueId("mexc".into()),
            to: VenueId("bingx".into()),
            tries: 0,
            why: mexc_16021().message,
        });
        arb::core::ledger::save(&core, &x).await;
    }
    let (core, j) = start(&d, None).await;
    let s = wait_state(&core, "r4DOGE", Duration::from_secs(10), |s| {
        matches!(s, UsdtReturn::NotReturned { .. })
    })
    .await;
    assert!(
        matches!(s, Some(UsdtReturn::NotReturned { why, .. }) if why.contains("все 1 повторов"))
    );
    assert_eq!(
        entries(&j, "core.usdt_return", "попытка 1 из 1")
            .await
            .len(),
        1
    );
}
