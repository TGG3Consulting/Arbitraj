//! Файл: tests/remeasure_relay_t77.rs
//! Что проверяет: решение 247, дополнение 10.11 (Тигран: согласовано; задача t77) — пере-замер продажи DEX в «Продать и
//! перегнать» и в автоматическом «Перегоне», на числах живого случая PIG 09.11 (сделка 20261109202626272PIG — факты
//! журнала бота из tests/remeasure_247.rs: куплено 7453353189 PIG, зачислено 7065220091).
//! 1) «Продать и перегнать» (core/manual_relay.rs): все её продажи — sell::market с причиной BUTTON → Guard::Any (без
//!    порога, решение 235); прочие причины по рынку — Guard::Off;
//! 2) продажа на цели сразу после автоматического «Перегона» (relay.rs → sell::sell_after_relay) — порог «выгодно»
//!    remeasure::PROFITABLE: граница — ровно итог сделки 0;
//! 3) налог, наблюдённый в сети, после перегона — по последнему переводу на площадку, где монета сейчас (сделка биржа ↔
//!    биржа, перегон с биржи на DEX), а не по первому переводу сделки.
//!
//! Не покрыто: сам вызов адаптера с TradeRequest.remeasure на живом ядре (в тестах ядро без площадок в реестре, моков нет).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{deal, remeasure, sell, types}.

use std::str::FromStr;

use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::json;

use arb::core::deal::Deal;
use arb::core::remeasure::{Guard, PROFITABLE, floor, of, tax_seen_pct};
use arb::core::sell::{BUTTON, guard_of};
use arb::core::types::{Asset, Direction, Transfer, TransferStatus, VenueId};

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn v(id: &str) -> VenueId {
    VenueId(id.into())
}

/// Сценарий на числах PIG (настоящая сделка была mexc → bsc_dex): сделка биржа ↔ биржа mexc → bingx, монета после
/// перегона — на DEX.
fn pig_relayed() -> Deal {
    let dir = Direction {
        asset: Asset {
            ticker: "PIG".into(),
            bsc_contract: None,
        },
        buy_on: v("mexc"),
        sell_on: v("bingx"),
    };
    let mut x = Deal::new(
        "20261109202626272PIG".into(),
        dir,
        json!(null),
        Decimal::ZERO,
    );
    x.at = v("bsc_dex");
    x.qty = d("7065220091");
    x.invested = d("99.6");
    x
}

fn transfer(venue: &str, amount: &str) -> Transfer {
    Transfer {
        venue: v(venue),
        asset: pig_relayed().direction.asset,
        network: "BSC".into(),
        amount: d(amount),
        status: TransferStatus::Credited,
        venue_ref: String::new(),
        tx_hash: None,
        fee: None,
        updated_at: Utc::now(),
    }
}

#[test]
fn sell_and_relay_sales_are_button_any() {
    // Все продажи «Продать и перегнать» идут sell::market(…, BUTTON): здесь, на цели, после несостоявшегося перегона.
    assert_eq!(guard_of(BUTTON), Guard::Any);
    // Прочие продажи по рынку (частичная покупка — executor.rs) — без пере-замера, как в решении 247.
    assert_eq!(
        guard_of("частичная покупка: продать на A выгоднее"),
        Guard::Off
    );
    let x = pig_relayed();
    assert_eq!(
        floor(&x, d("102.05"), d("102.10"), guard_of(BUTTON)),
        Some(Decimal::ZERO)
    );
}

#[test]
fn after_relay_floor_is_zero_result() {
    let x = pig_relayed();
    assert_eq!(PROFITABLE, Guard::MaxLoss(Decimal::ZERO));
    let (value, gross) = (d("102.05"), d("102.10"));
    let f = floor(&x, value, gross, PROFITABLE).unwrap();
    // На границе: чистыми f − (gross − value), итог сделки ровно 0 (условие «выгодно», решение 47).
    assert_eq!(x.result_if(f - (gross - value)), Decimal::ZERO);
    // Ручная продажа PIG 101.1307 — выше границы: повтор с новым minOut был бы разрешён.
    assert!(d("101.1307") >= f, "{f}");
}

#[test]
fn network_tax_after_relay_uses_last_transfer() {
    let mut x = pig_relayed();
    // Перевод сделки mexc → bingx без недостачи, затем перегон bingx → bsc_dex с недостачей PIG 09.11.
    x.transfers = vec![
        transfer("mexc", "7453353189"),
        transfer("bingx", "7453353189"),
        transfer("bingx", "7453353189"),
        transfer("bsc_dex", "7065220091"),
    ];
    let t = tax_seen_pct(&x).unwrap();
    assert!(t > d("5.2074") && t < d("5.2075"), "{t}");
    let r = of(&x, d("102.05"), d("102.10"), PROFITABLE).unwrap();
    assert_eq!(r.tax_seen_pct, Some(t));
    // Монета ещё на бирже после первого перевода (перегона не было) — недостачи нет, налога в сети нет.
    let mut y = pig_relayed();
    y.at = v("bingx");
    y.transfers = x.transfers[..2].to_vec();
    assert_eq!(tax_seen_pct(&y), None);
}
