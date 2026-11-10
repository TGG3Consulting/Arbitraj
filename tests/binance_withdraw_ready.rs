//! Файл: tests/binance_withdraw_ready.rs
//! Что проверяет: решение 251 (Binance: вывод сразу после покупки получал отказ) на примерах официальной документации
//! Binance (tests/data/binance_docs_2026-11-10, README там же), текстах отказов из сделок бота 10.11 и настоящем ядре
//! (SQLite во временной папке, без площадок в реестре):
//! (а) поток аккаунта: outboundAccountPosition → остатки (монета, свободно); ожидание доступности — сразу, если
//!     свободно ≥ суммы; ждёт событие и выходит по нему; вывод с funding — не сверяется; строки хронологии;
//! (б) −4026 / −4024 вывода в окне venue.withdraw_unready_window_ms после своей покупки → WithdrawLocked (повтор
//!     core/transfer_locked.rs, как −4060); вне окна, без окна, другой код — как раньше (WithdrawRejected);
//! (г) окончательный отказ вывода закрывает входы монеты с этой биржи (coin.entry_blocked_venues), другие биржи — нет;
//! (д) фактическая комиссия — transactionFee истории выводов; поправка затрат по фактам прошлого перевода.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::binance, arb::core (withdraw_wait, withdraw_block, withdraw_seen, transfer_locked).
//! Моков нет.

mod usdt_return_common;

use std::time::{Duration, Instant};

use rust_decimal::Decimal;
use serde_json::{Value, json};

use arb::core::op_error::OpError;
use arb::core::transfer_locked::{again, is_locked};
use arb::core::types::{Asset, Cost, CostKind, Direction, TransferStatus, VenueId, WithdrawReady};
use arb::gateway::errors::{ErrorCode, GatewayError};
use arb::settings::value::Scope;
use arb::venues::binance::account_update::{Event, parse};
use arb::venues::binance::errors::{Op, from_code};
use arb::venues::binance::{transfers, withdraw_ready};
use usdt_return_common::{dir, doge, start};

const DIR: &str = "tests/data/binance_docs_2026-11-10";

fn venue() -> VenueId {
    VenueId("binance".into())
}

/// Окружение адаптера Binance на настройках ядра. Ошибок нет.
fn bn(core: &arb::core::Core, d: &std::path::Path) -> arb::venues::binance::ctx::Ctx {
    let s = core.inner.deps.settings.clone();
    arb::venues::binance::ctx::Ctx {
        settings: s.clone(),
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        http: reqwest::Client::new(),
        journal: core.inner.deps.journal.clone(),
        raw: arb::storage::raw_log::RawLog::start(d.join("raw"), s),
        taps: Default::default(),
    }
}

async fn set(core: &arb::core::Core, name: &str, v: Value) {
    core.inner
        .deps
        .settings
        .set(name, Scope::Venue("binance".into()), Some(v), "тест")
        .await
        .unwrap();
}

/// Отказ вывода Binance кодом `code` с текстом из сделки бота (через настоящий разбор адаптера). Ошибок нет.
fn refusal(code: i64, msg: &str) -> GatewayError {
    let raw = json!({"code": code, "msg": msg}).to_string();
    from_code(code, msg, Op::Withdraw, &raw)
}

#[test]
fn account_position_doc_example() {
    let t = std::fs::read_to_string(format!("{DIR}/outbound_account_position.json")).unwrap();
    match parse(&t, &venue()) {
        Event::Position(list, at) => {
            assert_eq!(list, vec![("ETH".to_string(), Decimal::new(10_000, 0))]);
            assert_eq!(at, 1_564_034_571_105);
        }
        other => panic!("ожидался Position: {other:?}"),
    }
}

#[tokio::test]
async fn wait_returns_on_event_or_at_once() {
    let d = dir("bn251wait");
    let (core, _) = start(&d, None).await;
    let ctx = bn(&core, &d);
    set(&core, "venue.withdraw_wallet", json!("spot")).await;
    let coin = |t: &str| Asset {
        ticker: t.into(),
        bsc_contract: None,
    };
    let amount = Decimal::new(1234, 1);
    // Остаток уже есть — сразу.
    withdraw_ready::on_position(&[("W251A".into(), amount)], 1);
    let r = withdraw_ready::wait(&ctx, &coin("W251A"), amount, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(r.ready, Some(true));
    assert!(r.source.contains("outboundAccountPosition"), "{}", r.source);
    // Остатка нет — ждём событие (через 100 мс), выход по нему, не по сроку.
    withdraw_ready::on_position(&[("W251B".into(), Decimal::ONE)], 1);
    let t = Instant::now();
    let w = tokio::spawn({
        let ctx = ctx.clone();
        async move { withdraw_ready::wait(&ctx, &coin("W251B"), amount, Duration::from_secs(5)).await }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    // Старое событие (раньше записанного) не перезаписывает остаток.
    withdraw_ready::on_position(&[("W251B".into(), amount)], 0);
    assert_eq!(withdraw_ready::free("W251B"), Some(Decimal::ONE));
    withdraw_ready::on_position(&[("W251B".into(), amount + Decimal::ONE)], 2);
    let r = w.await.unwrap().unwrap();
    assert_eq!(r.ready, Some(true));
    assert_eq!(r.free, Some(amount + Decimal::ONE));
    assert!(t.elapsed() < Duration::from_secs(4), "вышли по событию");
    // Вывод с funding — остатки спота не сверяются.
    set(&core, "venue.withdraw_wallet", json!("funding")).await;
    let r = withdraw_ready::wait(&ctx, &coin("W251C"), amount, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(r.ready, None);
}

#[test]
fn chronology_lines() {
    use arb::core::withdraw_wait::line;
    let q = Decimal::new(1_234_567, 3);
    let ok = WithdrawReady {
        ready: Some(true),
        free: Some(Decimal::new(1_300_000, 3)),
        source: "событие баланса outboundAccountPosition".into(),
    };
    assert_eq!(
        line(412, "CYBER", q, Ok(ok)),
        "Ожидание доступности монеты для вывода: 412 мс — свободно 1300 ≥ 1234.567 CYBER (событие баланса outboundAccountPosition)"
    );
    let low = WithdrawReady {
        ready: Some(false),
        free: Some(Decimal::ZERO),
        source: "REST".into(),
    };
    assert!(line(3000, "CYBER", q, Ok(low)).contains("< 1234.567 CYBER (REST); вывод отправлен"));
    assert!(
        line(0, "X", q, Ok(WithdrawReady::default()))
            .contains("площадка доступность вывода не сообщает")
    );
    assert!(
        line(5, "X", q, Err("нет связи".into()))
            .contains("проверка не удалась (нет связи), вывод сразу")
    );
}

#[tokio::test]
async fn not_ready_codes_in_window_become_locked() {
    let d = dir("bn251codes");
    let (core, _) = start(&d, None).await;
    let ctx = bn(&core, &d);
    let e26 = refusal(-4026, "[031033] User has insufficient balance");
    let e24 = refusal(-4024, "[031031] User does not own this currency");
    assert_eq!(
        e26.code,
        ErrorCode::WithdrawRejected,
        "без окна — как раньше"
    );
    // Окно не задано — как раньше, даже сразу после покупки.
    withdraw_ready::bought("CYBER251");
    assert_eq!(
        transfers::not_ready(&ctx, "CYBER251", e26.clone()).code,
        ErrorCode::WithdrawRejected
    );
    set(&core, "venue.withdraw_unready_window_ms", json!(5000)).await;
    for e in [&e26, &e24] {
        let x = transfers::not_ready(&ctx, "CYBER251", e.clone());
        assert_eq!(x.code, ErrorCode::WithdrawLocked, "{}", x.message);
        assert!(
            x.message.contains("ещё не доступна для вывода"),
            "{}",
            x.message
        );
        assert!(is_locked(&x), "вывод не создан — повтор безопасен");
    }
    // Своей покупки монеты не было — как раньше.
    assert_eq!(
        transfers::not_ready(&ctx, "WLFI251", e26.clone()).code,
        ErrorCode::WithdrawRejected
    );
    // Другой отказ вывода (адрес не в белом списке −4035) — как раньше.
    let wl = refusal(-4035, "This address is not on the whitelist.");
    assert_eq!(
        transfers::not_ready(&ctx, "CYBER251", wl).code,
        ErrorCode::WithdrawRejected
    );
    // Вне окна — как раньше.
    set(&core, "venue.withdraw_unready_window_ms", json!(1)).await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(
        transfers::not_ready(&ctx, "CYBER251", e26.clone()).code,
        ErrorCode::WithdrawRejected
    );
    // Повтор ядра — по transfer.locked_* площадки (как −4060).
    set(&core, "transfer.locked_retry_ms", json!(20)).await;
    set(&core, "transfer.locked_retries_max", json!(2)).await;
    set(&core, "venue.withdraw_unready_window_ms", json!(60_000)).await;
    let id = "20261110120000000CYBER";
    arb::core::ledger::save(&core, &doge(id)).await;
    let x = transfers::not_ready(&ctx, "CYBER251", e26);
    let mut done = 0;
    while again(&core, id, &venue(), &x, &mut done).await {}
    assert_eq!(done, 2);
}

#[tokio::test]
async fn refusal_closes_entries_from_that_exchange_only() {
    let d = dir("bn251block");
    let (core, j) = start(&d, None).await;
    let cyber = Asset {
        ticker: "CYBER".into(),
        bsc_contract: Some(alloy_primitives::address!(
            "0x14778860e937f509e651192a90589de711fb88a9"
        )),
    };
    let dir_of = |v: &str| Direction {
        asset: cyber.clone(),
        buy_on: VenueId(v.into()),
        sell_on: VenueId("bsc_dex".into()),
    };
    assert!(arb::core::withdraw_block::check(&core, &dir_of("binance")).is_ok());
    let e = OpError::from(&refusal(-4026, "[031033] User has insufficient balance"));
    arb::core::withdraw_block::close(&core, "20261110120000000CYBER", &cyber, &venue(), &e).await;
    let why = arb::core::withdraw_block::check(&core, &dir_of("binance")).unwrap_err();
    assert!(why.contains("входы с binance закрыты"), "{why}");
    assert!(
        why.contains("[031033] User has insufficient balance"),
        "{why}"
    );
    assert!(
        arb::core::withdraw_block::check(&core, &dir_of("bingx")).is_ok(),
        "другая биржа — открыта"
    );
    // Второй отказ той же биржи — не дублирует её в списке.
    arb::core::withdraw_block::close(&core, "x", &cyber, &venue(), &e).await;
    let scope = arb::core::blacklist::coin_scope(&cyber);
    let id = match &scope {
        Scope::Coin(id) => id.clone(),
        _ => unreachable!(),
    };
    let at = arb::settings::read::At {
        venue: None,
        coin: Some(&id),
    };
    let list = core
        .inner
        .deps
        .settings
        .list(arb::core::withdraw_block::VENUES, at)
        .unwrap();
    assert_eq!(list, vec!["binance".to_string()]);
    // Отказ не из refusal (ордер) — не закрывает.
    let other = OpError::from(&GatewayError::new(ErrorCode::OrderRejected, None, "ордер"));
    arb::core::withdraw_block::close(&core, "y", &cyber, &VenueId("mexc".into()), &other).await;
    assert!(arb::core::withdraw_block::check(&core, &dir_of("mexc")).is_ok());
    assert!(
        !usdt_return_common::entries(&j, "core.withdraw_block", "CYBER")
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn history_fee_and_seen_cost() {
    let d = dir("bn251fee");
    let (core, _) = start(&d, None).await;
    let ctx = bn(&core, &d);
    let list: Value = serde_json::from_str(
        &std::fs::read_to_string(format!("{DIR}/withdraw_history.json")).unwrap(),
    )
    .unwrap();
    let rec = &list[0];
    let t = transfers::transfer(
        &ctx,
        rec,
        "b6ae22b3aa844210a7041aee7589627c",
        TransferStatus::Sent,
    )
    .unwrap();
    let fee = t.fee.expect("transactionFee");
    assert_eq!(
        (fee.kind, fee.amount, fee.asset.as_str()),
        (CostKind::WithdrawFee, Decimal::new(4, 3), "USDT")
    );
    // Поправка по фактам: комиссия по факту 0.5 больше справочника 0.3, недостача 2 % сверх налога 1 %.
    let coin = Asset {
        ticker: "CYBER".into(),
        bsc_contract: None,
    };
    let seen = arb::core::withdraw_seen::Seen {
        asset: coin.clone(),
        from: venue(),
        fee: Some(Decimal::new(5, 1)),
        shortfall_pct: Decimal::TWO,
    };
    let amount = Decimal::from(100);
    let dir_costs = vec![
        Cost::new(CostKind::WithdrawFee, Decimal::new(3, 1), "CYBER", false),
        Cost::new(CostKind::TransferTax, Decimal::new(997, 3), "CYBER", false),
    ];
    let extra = arb::core::withdraw_seen::extra(&seen, amount, &dir_costs);
    assert_eq!(extra.len(), 2);
    assert_eq!(
        (extra[0].kind, extra[0].amount),
        (CostKind::WithdrawFee, Decimal::new(2, 1))
    );
    // (100 − 0.5 − 0.997) × 2 % = 1.97006.
    assert_eq!(
        (extra[1].kind, extra[1].amount),
        (CostKind::TransferTax, Decimal::new(197_006, 5))
    );
    // Факт не больше справочника и недостачи нет — поправки нет.
    let same = arb::core::withdraw_seen::Seen {
        fee: Some(Decimal::new(3, 1)),
        shortfall_pct: Decimal::ZERO,
        ..seen.clone()
    };
    assert!(arb::core::withdraw_seen::extra(&same, amount, &dir_costs).is_empty());
    // В расчёт — только при transfer.seen_cost площадки.
    arb::core::withdraw_seen::record(&core, seen).await;
    assert!(
        arb::core::withdraw_seen::adjust(&core, &coin, &venue(), amount, &dir_costs).is_empty()
    );
    set(&core, "transfer.seen_cost", json!(true)).await;
    assert_eq!(
        arb::core::withdraw_seen::adjust(&core, &coin, &venue(), amount, &dir_costs),
        extra
    );
}
