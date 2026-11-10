//! Файл: tests/remeasure_first_sale_t78.rs
//! Что проверяет: решение 247, дополнение 10.11 (задача t78, Тигран: согласовано) — пере-замер продажи DEX при откате
//! по minOut и в оставшихся местах продажи монет сделки по правилу «итог сделки не ниже нуля» (remeasure::PROFITABLE):
//! первая продажа сделки и «Вернуть боту» (sell::sell_or_stuck), «Ожидание» по истечении stuck.wait_ms, продажа после
//! частичной покупки. Сценарий на числах PIG 09.11 (сделка 20261109202626272PIG, mexc → bsc_dex, первая продажа на DEX —
//! факты журнала бота из tests/remeasure_247.rs: «execution reverted: minOut; ожидалось 102.101336 USDT, модельный
//! налог 4.0003%», налог перевода в сети 5.0 %, вложено ~99.6):
//! 1) первая продажа откатилась по minOut → граница ядра PROFITABLE (итог 0) → граница выхода адаптера по замеру и
//!    налогу в сети (bound_text) не ниже её → повтор с minOut от замера (min_out_of), продажа при итоге ≥ 0;
//! 2) итог < 0 (вложено условно 101.5 — больше выхода по замеру) → граница выхода ниже границы ядра → повтора нет:
//!    отказ продажи, дальше — режим застрявшей монеты, как раньше (core/sell.rs → sell_unsent / stuck);
//! 3) кнопки человека — без порога (Guard::Any), как раньше.
//!
//! Сравнение «граница выхода < remeasure.floor — отказ» — то же, что в exec/executor/remeasure.rs и exec/ur_remeasure.rs.
//!
//! Не покрыто: сам вызов адаптера на живом ядре (в тестах ядро без площадок в реестре, моков нет).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{deal, remeasure, sell, types}, arb::venues::bsc_dex::exec::executor::remeasure.

use std::str::FromStr;

use alloy_primitives::U256;
use rust_decimal::Decimal;
use serde_json::json;

use arb::core::deal::Deal;
use arb::core::remeasure::{Guard, PROFITABLE, floor};
use arb::core::sell::{BUTTON, guard_of, profitable};
use arb::core::types::{Asset, Direction, VenueId};
use arb::venues::bsc_dex::exec::executor::remeasure::{bound_text, min_out_of};

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

/// Первая продажа PIG на DEX (сделка mexc → bsc_dex), монеты на кошельке сделки.
fn pig(invested: &str) -> Deal {
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
    x.invested = d(invested);
    x
}

/// Граница выхода адаптера после отката по minOut: модель 102.101336 USDT при налоге 4.0003 %, налог в сети 5.0 %,
/// замер (симуляция) 102.161649 — как в журнале PIG (tests/remeasure_247.rs).
fn bound() -> Decimal {
    let exp = [(d("102.101336"), d("4.0003"))];
    bound_text(&exp, d("102.161649"), Some(d("5.0"))).0
}

#[test]
fn first_sale_rolled_back_resells_when_result_not_negative() {
    let x = pig("99.6");
    // Затраты вне цены у котировки — 0.05 (как в tests/remeasure_247.rs): «выгодно» → sell_or_stuck продаёт.
    let (value, gross) = (d("102.05"), d("102.10"));
    assert!(profitable(&x, value));
    let f = floor(&x, value, gross, PROFITABLE).unwrap();
    // На границе итог сделки ровно 0 (решение 47).
    assert_eq!(x.result_if(f - (gross - value)), Decimal::ZERO);
    // Граница выхода по налогу в сети 101.038… ≥ границы ядра 99.65 — повтор с minOut от неё.
    let b = bound();
    assert!(b >= f, "граница {b}, порог {f}");
    assert!(x.result_if(b - (gross - value)) >= Decimal::ZERO);
    // minOut повтора = граница × (1 − 0.5 %), ниже старого minOut модели (101.590829).
    let m = min_out_of(b, d("0.5"), 6).unwrap();
    assert!(m < U256::from(101_590_829u64), "{m}");
    assert!(m > U256::ZERO);
}

#[test]
fn first_sale_rolled_back_no_resell_when_result_negative() {
    // Вложено условно 101.5: при выходе по замеру итог < 0 — повтора нет (отказ, режим застрявшей монеты, как раньше).
    let x = pig("101.5");
    let (value, gross) = (d("102.05"), d("102.10"));
    let f = floor(&x, value, gross, PROFITABLE).unwrap();
    let b = bound();
    assert!(b < f, "граница {b}, порог {f}");
    assert!(x.result_if(b - (gross - value)) < Decimal::ZERO);
}

#[test]
fn buttons_stay_without_threshold() {
    let x = pig("101.5");
    assert_eq!(guard_of(BUTTON), Guard::Any);
    assert_eq!(
        floor(&x, d("102.05"), d("102.10"), guard_of(BUTTON)),
        Some(Decimal::ZERO)
    );
    // Порог «выгодно» — порог убытка 0 (тот же, что у sell_after_relay, t77).
    assert_eq!(PROFITABLE, Guard::MaxLoss(Decimal::ZERO));
}
