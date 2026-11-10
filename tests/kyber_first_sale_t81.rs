//! Файл: tests/kyber_first_sale_t81.rs
//! Что проверяет: t81 — дополнение к решению 247 п. Г (Тигран 10.11: согласовано). Пути, где пере-замер добавил t78
//! (первая продажа сделки, «Вернуть боту», частичная покупка, «Ожидание» по истечении stuck.wait_ms), — порог ядра
//! remeasure::PROFITABLE_OPEN: та же граница, что у PROFITABLE, и признак Remeasure.any_executor; остальные пороги
//! (кнопка — Guard::Any, «Порог убытка», «Перегон», «Ожидание: стало выгодно», продажа после перегона t77 — PROFITABLE)
//! — без признака. Выбор пути адаптера DEX (venues/bsc_dex/trade_path.rs, его вызывает venue.rs trade_logs):
//! 1) признак есть: exec.via_kyber и «лучший из двух» — BestOf; только exec.via_kyber — Kyber; exec.via_kyber = нет — Own;
//! 2) пере-замер без признака (пути решения 247) — Own при любых настройках (п. Г, как было);
//! 3) без пере-замера (покупка, «Продать» в «Балансах») и не продажа выхода — по настройкам, как раньше.
//!
//! Числа сделки — PIG 09.11 (tests/remeasure_first_sale_t78.rs).
//!
//! Не покрыто: какие вызовы ядра ставят PROFITABLE_OPEN (sell::sell_or_stuck, executor.rs частичная покупка, stuck.rs
//! по истечении) — факт кода, на живом ядре без площадок в реестре не вызвать (моков нет); исполнение KyberSwap и
//! «лучшего из двух» — сеть.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{deal, remeasure, sell, types}, arb::venues::bsc_dex::trade_path.

use std::str::FromStr;

use rust_decimal::Decimal;
use serde_json::json;

use arb::core::deal::Deal;
use arb::core::remeasure::{Guard, PROFITABLE, PROFITABLE_OPEN, floor, of};
use arb::core::sell::{BUTTON, guard_of};
use arb::core::types::{Asset, Direction, Remeasure, Side, TradeRequest, VenueId};
use arb::venues::bsc_dex::trade_path::{self, Path};

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn pig() -> Deal {
    let dir = Direction {
        asset: Asset {
            ticker: "PIG".into(),
            bsc_contract: None,
        },
        buy_on: VenueId("mexc".into()),
        sell_on: VenueId("bsc_dex".into()),
    };
    let mut x = Deal::new(
        "20261109202626272PIG".into(),
        dir,
        json!(null),
        Decimal::ZERO,
    );
    x.at = VenueId("bsc_dex".into());
    x.qty = d("7065220091");
    x.invested = d("99.6");
    x
}

/// Продажа выхода PIG на DEX с условиями пере-замера `rm`.
fn sale(exit: bool, rm: Option<Remeasure>) -> TradeRequest {
    TradeRequest {
        venue: VenueId("bsc_dex".into()),
        asset: pig().direction.asset,
        side: Side::Sell,
        qty: d("7065220091"),
        worst_price: d("0.0000143"),
        deal_id: "20261109202626272PIG".into(),
        route: None,
        exit_position: exit,
        remeasure: rm,
    }
}

const VALUE: &str = "102.05";
const GROSS: &str = "102.10";

#[test]
fn open_guard_same_floor_and_marks_any_executor() {
    let x = pig();
    let (v, g) = (d(VALUE), d(GROSS));
    // Граница та же, что у PROFITABLE (итог сделки 0).
    assert_eq!(
        floor(&x, v, g, PROFITABLE_OPEN),
        floor(&x, v, g, PROFITABLE)
    );
    assert_eq!(PROFITABLE_OPEN, Guard::Open(Decimal::ZERO));
    let r = of(&x, v, g, PROFITABLE_OPEN).unwrap();
    assert!(r.any_executor);
    // Прочие пороги — без признака (пути решения 247: только свой сборщик).
    for gd in [PROFITABLE, guard_of(BUTTON), Guard::MaxLoss(d("3"))] {
        assert!(!of(&x, v, g, gd).unwrap().any_executor, "{gd:?}");
    }
    assert_eq!(of(&x, v, g, Guard::Off), None);
}

#[test]
fn t78_paths_go_as_before_t78() {
    let x = pig();
    let rm = of(&x, d(VALUE), d(GROSS), PROFITABLE_OPEN);
    let req = sale(true, rm);
    assert_eq!(trade_path::of(&req, true, true), Path::BestOf);
    assert_eq!(trade_path::of(&req, true, false), Path::Kyber);
    assert_eq!(trade_path::of(&req, false, false), Path::Own);
    // Пере-замер при этом в запросе остаётся — его получит свой сборщик (откат с KyberSwap, своя сторона best_of).
    assert!(req.remeasure.is_some());
}

#[test]
fn decision_247_paths_stay_own_only() {
    let x = pig();
    for gd in [PROFITABLE, guard_of(BUTTON), Guard::MaxLoss(d("3"))] {
        let req = sale(true, of(&x, d(VALUE), d(GROSS), gd));
        assert_eq!(trade_path::of(&req, true, true), Path::Own, "{gd:?}");
        assert_eq!(trade_path::of(&req, true, false), Path::Own, "{gd:?}");
        assert_eq!(trade_path::of(&req, false, false), Path::Own, "{gd:?}");
    }
}

#[test]
fn without_remeasure_by_settings_as_before() {
    for req in [sale(true, None), sale(false, None)] {
        assert_eq!(trade_path::of(&req, true, true), Path::BestOf);
        assert_eq!(trade_path::of(&req, true, false), Path::Kyber);
        assert_eq!(trade_path::of(&req, false, false), Path::Own);
    }
}

#[test]
fn old_request_json_without_flag_is_own_only() {
    // Запрос без поля any_executor (до t81) — признака нет: только свой сборщик (п. Г).
    let rm: Remeasure = serde_json::from_value(json!({"floor": "99.65"})).unwrap();
    assert!(!rm.any_executor);
    assert_eq!(trade_path::of(&sale(true, Some(rm)), true, true), Path::Own);
}
