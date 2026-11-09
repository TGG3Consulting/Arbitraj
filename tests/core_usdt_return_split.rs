//! Файл: tests/core_usdt_return_split.rs
//! Что делает: решение 227 на числах живой сделки 20261108104958308JCT (bingx → bitget): частичная продажа на Bitget
//! 21943.3 по 0.006248 (≈137.10 USDT), остаток перегнан на DEX (кошелёк w7) и продан там (≈342.29 USDT). Проверяется:
//! (1) чистая логика частей — с каждой площадки выручки ровно её выручка минус комиссия в котируемой монете, первая —
//! площадка закрытия, площадка покупки не часть; ключи частей и их разбор; «закончено» — только когда все части;
//! (2) сумма повтора — ровно сумма части: при свободных 387.56 на w7 уходит 342.29, а не всё; свободно меньше — отказ;
//! (3) закрытие на настоящей SQLite без площадок в реестре (моков нет): две части, каждая своим состоянием и своей
//! записью «USDT не вернулись»; «Перегнано вручную» одной части не трогает другую;
//! (4) метка «вывод с площадки отклонён» (дополнение к 227) — чистая логика;
//! (5) тест-страж по исходникам: перегон застрявшей монеты идёт через transfers::send_trade (зачисление на DEX по
//! приходу в блокчейне, решение 151), а не transfers::send.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::gateway, arb::settings, arb::storage, arb::notify, arb::secrets.

mod usdt_return_common;

use std::str::FromStr;
use std::time::Duration;

use arb::core::deal::{Deal, Step};
use arb::core::op_error::OpError;
use arb::core::types::{
    CommandSource, DexAccountId, Fee, ManualAction, ManualCommand, Side, TradeRequest, TradeResult,
    TradeStatus, VenueId,
};
use arb::core::usdt_return_parts as parts;
use arb::core::usdt_return_state::UsdtReturn;
use arb::gateway::errors::{ErrorCode, GatewayError};
use rust_decimal::Decimal;
use usdt_return_common::{dir, doge, start};

/// Число из строки. Ошибок нет (тестовые литералы).
fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

/// Площадка по имени.
fn v(s: &str) -> VenueId {
    VenueId(s.into())
}

/// Продажа бота: qty по avg на площадке, комиссия fees. Ошибок нет.
fn sell(venue: &str, qty: Decimal, avg: Decimal, fees: Vec<Fee>) -> TradeResult {
    TradeResult {
        request: TradeRequest {
            venue: v(venue),
            asset: doge("x").direction.asset,
            side: Side::Sell,
            qty,
            worst_price: avg,
            deal_id: "x".into(),
            route: None,
            exit_position: true,
        },
        status: TradeStatus::Filled,
        filled_qty: qty,
        avg_price: avg,
        fees,
        venue_ref: "0x".into(),
        chain: None,
        finished_at: chrono::Utc::now(),
        executor: None,
    }
}

/// Сделка как JCT: куплено на bingx, часть продана на `cex` (комиссия 0.1371 USDT), остаток — на bsc_dex (кошелёк w7)
/// на 342.29 USDT без комиссии в USDT; сделка закрылась на bsc_dex. Ошибок нет.
fn jct(id: &str, cex: &str) -> Deal {
    let mut x = doge(id);
    x.direction.buy_on = v("bingx");
    x.direction.sell_on = v(cex);
    x.at = v("bsc_dex");
    x.dex_account = Some(DexAccountId("w7".into()));
    x.invested = d("470");
    let fee = Fee {
        asset: "USDT".into(),
        amount: d("0.1371"),
        usd: None,
    };
    let a = sell(cex, d("21943.3"), d("0.006248"), vec![fee]);
    let b = sell("bsc_dex", d("54783.8"), d("0.00624800"), vec![]);
    x.proceeds = a.filled_qty * a.avg_price + b.filled_qty * b.avg_price;
    x.sells = vec![a, b];
    x.qty = Decimal::ZERO;
    x
}

#[test]
fn jct_split_is_per_venue_proceeds_minus_fee() {
    let x = jct("s1JCT", "bitget");
    let dex = d("54783.8") * d("0.006248");
    let cex = d("21943.3") * d("0.006248") - d("0.1371");
    let got = parts::split(&x, |_| Some("USDT".into()));
    // Первая часть — площадка закрытия (bsc_dex), вторая — Bitget; не одна сумма всей выручки с DEX.
    assert_eq!(got, vec![(v("bsc_dex"), dex), (v("bitget"), cex)]);
    assert!(parts::multi(&x));
    // Площадка покупки не часть: выручка там уже дома.
    let mut y = x.clone();
    y.sells[0].request.venue = v("bingx");
    assert_eq!(
        parts::split(&y, |_| Some("USDT".into())),
        vec![(v("bsc_dex"), dex)]
    );
}

#[test]
fn part_keys_and_all_done() {
    let mut x = jct("s2JCT", "bitget");
    let p = |at: &str| UsdtReturn::Pending {
        amount: d("1"),
        from: v(at),
        to: v("bingx"),
    };
    parts::put(&mut x, p("bsc_dex"));
    parts::put(&mut x, p("bitget"));
    assert_eq!(parts::key(&x, &v("bsc_dex")), "s2JCT");
    assert_eq!(parts::key(&x, &v("bitget")), "s2JCT@bitget");
    assert_eq!(parts::parse("s2JCT@bitget"), ("s2JCT", Some(v("bitget"))));
    assert_eq!(parts::parse("s2JCT"), ("s2JCT", None));
    let done = |at: &str| UsdtReturn::Done {
        amount: d("1"),
        credited: d("1"),
        from: v(at),
        to: v("bingx"),
        sent: None,
        fee_usd: None,
    };
    parts::put(&mut x, done("bsc_dex"));
    // Одна часть вернулась — возврат сделки ещё не закончен.
    assert!(!parts::all_done(&x));
    parts::put(&mut x, done("bitget"));
    assert!(parts::all_done(&x));
    assert_eq!(parts::all(&x).len(), 2);
}

#[test]
fn retry_sends_exactly_the_part_never_more() {
    // JCT: на w7 свободно 387.56 (342.29 выручки + 45.28 оборотных другого направления) — уходит только часть.
    assert_eq!(
        parts::send_amount(d("387.56"), d("342.29"), None),
        Ok(d("342.29"))
    );
    // Свободно меньше части — ничего не отправляется (ждать / «не вернулись»), чужое не забирается.
    let e = parts::send_amount(d("300"), d("342.29"), None).unwrap_err();
    assert!(e.contains("чужие USDT не трогаем"), "{e}");
    assert!(parts::send_amount(d("10"), Decimal::ZERO, None).is_err());
}

#[tokio::test]
async fn jct_close_starts_two_parts_each_with_own_state() {
    let (core, _j) = start(&dir("s227close"), None).await;
    // mexc — в роли Bitget (у mexc в общем ядре теста задана котируемая монета USDT).
    let mut x = jct("s3JCT", "mexc");
    x.step = Step::Selling;
    arb::core::ledger::save(&core, &x).await;
    arb::core::ledger::finish(&core, &mut x, Step::Closed, "продано (тест)").await;
    let dex = d("54783.8") * d("0.006248");
    let cex = d("21943.3") * d("0.006248") - d("0.1371");
    // Площадок нет — обе попытки не состоялись, повторов нет: две части «не вернулись», каждая на свою сумму.
    let t = std::time::Instant::now();
    let deal = loop {
        let y = core.deals().into_iter().find(|y| y.id == "s3JCT").unwrap();
        let all_nr = parts::all(&y).len() == 2
            && parts::all(&y)
                .iter()
                .all(|p| matches!(p, UsdtReturn::NotReturned { .. }));
        if all_nr || t.elapsed() > Duration::from_secs(5) {
            break y;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    match (&deal.usdt_return, deal.usdt_return_parts.as_slice()) {
        (
            Some(UsdtReturn::NotReturned {
                amount: a, at: va, ..
            }),
            [
                UsdtReturn::NotReturned {
                    amount: b, at: vb, ..
                },
            ],
        ) => {
            assert_eq!((va, *a), (&v("bsc_dex"), dex));
            assert_eq!((vb, *b), (&v("mexc"), cex));
        }
        other => panic!("ждали две части «не вернулись», а {other:?}"),
    }
    let keys: Vec<String> = core
        .manual_records()
        .into_iter()
        .filter(|r| r.deal_id == "s3JCT")
        .map(|r| r.key)
        .collect();
    assert_eq!(
        keys,
        vec!["u.s3JCT".to_string(), "u.s3JCT@mexc".to_string()]
    );
    // «Перегнано вручную» части с mexc — только она «вручную», часть DEX по-прежнему ждёт.
    core.command(ManualCommand {
        action: ManualAction::UsdtReturnedManually {
            deal_id: "s3JCT@mexc".into(),
            usdt: None,
        },
        source: CommandSource::Ui,
        by: Some("тест".into()),
        at: chrono::Utc::now(),
    })
    .await
    .unwrap();
    let y = core.deals().into_iter().find(|y| y.id == "s3JCT").unwrap();
    assert!(matches!(
        parts::get(&y, Some(&v("mexc"))),
        Some(UsdtReturn::Manual { .. })
    ));
    assert!(matches!(
        y.usdt_return,
        Some(UsdtReturn::NotReturned { .. })
    ));
    assert!(!parts::all_done(&y));
}

#[test]
fn old_deal_without_parts_field_still_loads() {
    let mut j = serde_json::to_value(doge("s4DOGE")).unwrap();
    j.as_object_mut().unwrap().remove("usdt_return_parts");
    j.as_object_mut().unwrap().remove("withdraw_refused");
    let x: Deal = serde_json::from_value(j).unwrap();
    assert!(x.usdt_return_parts.is_empty());
    assert!(x.withdraw_refused.is_empty());
}

#[test]
fn withdraw_refused_mark_blocks_only_refusal_codes() {
    use arb::core::withdraw_refused::{mark, refused};
    let mut x = jct("s5JCT", "bitget");
    let err = |c: ErrorCode| OpError::from(&GatewayError::new(c, Some(v("bitget")), "отказ"));
    // Сеть/таймаут — не метка: вывод с Bitget ещё можно пробовать.
    assert!(!mark(&mut x, &v("bitget"), &err(ErrorCode::Timeout)));
    assert!(!refused(&x, &v("bitget")));
    for c in [
        ErrorCode::WithdrawRejected,
        ErrorCode::InvalidAddress,
        ErrorCode::WithdrawLocked,
    ] {
        let mut y = x.clone();
        assert!(mark(&mut y, &v("bitget"), &err(c)));
        assert!(refused(&y, &v("bitget")));
        assert!(!refused(&y, &v("bsc_dex")));
    }
    // Повторная метка не дублирует площадку.
    mark(&mut x, &v("bitget"), &err(ErrorCode::WithdrawRejected));
    mark(&mut x, &v("bitget"), &err(ErrorCode::WithdrawRejected));
    assert_eq!(x.withdraw_refused, vec![v("bitget")]);
}

#[test]
fn relay_uses_chain_credit_send_trade() {
    let s =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/core/relay.rs")).unwrap();
    assert!(
        s.contains("transfers::send_trade("),
        "перегон — через send_trade (решение 151/227)"
    );
    assert!(
        !s.contains("transfers::send("),
        "перегон не должен ждать хеш биржи (transfers::send)"
    );
}
