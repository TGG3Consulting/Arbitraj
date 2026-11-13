//! Файл: tests/core_window_rejects_266.rs
//! Что проверяет: решение 266, ответы Тиграна 13.11 (чистая логика, без сети и диска):
//! 1) пауза направления после отказа ордера покупки (executor::buy_refusal_pauses): биржа A отклонила ордер с кодом
//!    шлюза «не хватает средств» (InsufficientBalance) — паузы нет; другой код, ошибка без кода, покупка на DEX — пауза
//!    как раньше; пометка записи закрытия;
//! 2) отказы предпроверки сделки в окне монеты (window.rs note_reject): повтор той же причины (вид текста без чисел —
//!    reject_shape.rs) по тому же направлению — одна запись со счётчиком, первым и последним временем; другая причина
//!    или направление — своя запись; метка «баланс»; окна монеты нет — не пишется; отказы уходят в сводку закрытого окна;
//! 3) сводка окна без поля rejects (старые окна в базе и файлах) читается — пустой список; с отказами — туда и обратно;
//! 4) какие отказы предпроверки считаются отказами «из-за свободного баланса» (precheck_balance::is_balance).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{executor, op_error, window, reject_shape, precheck_balance, limits}, arb::gateway.

use arb::core::coin_key::CoinKey;
use arb::core::executor::{NO_PAUSE, buy_refusal_pauses};
use arb::core::op_error::OpError;
use arb::core::precheck_balance::{self, Cand};
use arb::core::reject_shape::shape;
use arb::core::types::{Asset, Direction, VenueId};
use arb::core::window::{Pass, WindowInfo, Windows};
use arb::gateway::contract::VenueKind;
use arb::gateway::errors::{ErrorCode, GatewayError};
use chrono::{Duration, Utc};
use rust_decimal::Decimal;

/// Площадка по id.
fn v(id: &str) -> VenueId {
    VenueId(id.into())
}

/// Направление по монете без адреса (ключ монеты — тикер).
fn dir(buy: &str, sell: &str) -> Direction {
    Direction {
        asset: Asset {
            ticker: "CAKE".into(),
            bsc_contract: None,
        },
        buy_on: v(buy),
        sell_on: v(sell),
    }
}

#[test]
fn no_direction_pause_only_for_exchange_no_funds_on_buy() {
    let err = |code| OpError::from(&GatewayError::new(code, Some(v("bingx")), "отказ"));
    let no_funds = err(ErrorCode::InsufficientBalance);
    assert!(!buy_refusal_pauses(VenueKind::Cex, &no_funds));
    // Покупка на DEX с тем же кодом, другие коды, ошибка ядра без кода — пауза как раньше.
    assert!(buy_refusal_pauses(VenueKind::Dex, &no_funds));
    for code in [
        ErrorCode::OrderRejected,
        ErrorCode::OrderNotFilled,
        ErrorCode::RateLimited,
        ErrorCode::Timeout,
    ] {
        assert!(buy_refusal_pauses(VenueKind::Cex, &err(code)), "{code:?}");
    }
    assert!(buy_refusal_pauses(
        VenueKind::Cex,
        &OpError::from("нет худшего уровня плана".to_string())
    ));
    assert!(NO_PAUSE.contains("пауза направления не поставлена"));
    assert!(NO_PAUSE.contains("нехватки средств"));
}

#[test]
fn precheck_rejects_are_kept_in_window_with_counter() {
    let mut w = Windows::default();
    let d = dir("bingx", "bsc_dex");
    let t0 = Utc::now();
    // Окна монеты нет — отказ не пишется.
    let r1 = "bingx: CAKE вывод закрыт (время: справочник сетей bingx 12 мс)";
    assert!(!w.note_reject(&d, shape(r1), r1, false, t0));
    let p = w.pass(&d, "w1".into(), false, Decimal::ONE, &[v("bingx")], t0);
    assert!(matches!(p, Pass::Opened(_)));
    // Та же причина с другими числами — одна запись, счётчик, первое и последнее время, текст — последний.
    let r2 = "bingx: CAKE вывод закрыт (время: справочник сетей bingx 340 мс)";
    assert_eq!(shape(r1), shape(r2));
    assert!(w.note_reject(&d, shape(r1), r1, false, t0));
    let t1 = t0 + Duration::milliseconds(500);
    assert!(w.note_reject(&d, shape(r2), r2, false, t1));
    // Отказ из-за баланса — своя запись с меткой; другое направление той же монеты — своя запись.
    let c = Cand {
        qty: Decimal::TEN,
        spend_max: Decimal::from(25),
        profit_usd: Decimal::ONE,
    };
    let bal =
        precheck_balance::short_reject(&d, Decimal::from(5), "USDT", &c, "прибыль ниже порога");
    assert!(precheck_balance::is_balance(&d, &bal));
    assert!(w.note_reject(&d, shape(&bal), &bal, true, t1));
    let other = dir("mexc", "bsc_dex");
    assert!(w.note_reject(&other, shape(r1), r1, false, t1));
    let info = w.get(&CoinKey::of(&d.asset)).unwrap();
    assert_eq!(info.rejects.len(), 3);
    let first = &info.rejects[0];
    assert_eq!((first.count, first.first_at, first.last_at), (2, t0, t1));
    assert_eq!((first.reason.as_str(), first.balance), (r2, false));
    assert_eq!(first.direction, d);
    assert!(info.rejects[1].balance && info.rejects[1].count == 1);
    assert_eq!(info.rejects[2].direction, other);
    // Отказы — в сводке закрытого окна.
    let closed = w.close(&CoinKey::of(&d.asset), "тест", t1).unwrap();
    assert_eq!(closed.rejects, info.rejects);
    // Сводка с отказами — в JSON и обратно; сводка без поля (окна до решения 266) — пустой список.
    let json = serde_json::to_string(&closed).unwrap();
    let back: WindowInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(back, closed);
    let mut old = serde_json::to_value(&closed).unwrap();
    assert!(old.as_object_mut().unwrap().remove("rejects").is_some());
    let old: WindowInfo = serde_json::from_value(old).unwrap();
    assert!(old.rejects.is_empty());
    // Окно без отказов поле не пишет вовсе — формат строк прежних окон не меняется.
    let mut w = Windows::default();
    w.pass(&d, "w2".into(), false, Decimal::ONE, &[v("bingx")], t0);
    let clean = serde_json::to_value(w.get(&CoinKey::of(&d.asset)).unwrap()).unwrap();
    assert!(clean.get("rejects").is_none());
}

#[test]
fn balance_rejects_are_told_from_others() {
    let d = dir("bingx", "bsc_dex");
    let zero = arb::core::limits::none_free(&d.buy_on, "USDT");
    assert_eq!(zero, "на bingx свободно 0 USDT");
    for yes in [
        zero.as_str(),
        "свободный баланс bingx: свободно 5 USDT, кандидату нужно 25 — на свободный остаток 5 сделка невыгодна: …",
        "свободный баланс bingx не получен: узел не ответил",
    ] {
        assert!(precheck_balance::is_balance(&d, yes), "{yes}");
    }
    for no in [
        "bingx: CAKE вывод закрыт",
        "пересчёт перед покупкой: прибыль 0.01 USDT ниже порога 0.1",
        "на mexc свободно 0 USDT",
        "свободный баланс mexc не получен: …",
    ] {
        assert!(!precheck_balance::is_balance(&d, no), "{no}");
    }
}
