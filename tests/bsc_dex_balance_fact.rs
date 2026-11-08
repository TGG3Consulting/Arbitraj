//! Файл: tests/bsc_dex_balance_fact.rs
//! Что делает: решение 222 — факт продажи DEX по изменению баланса кошелька (venues/bsc_dex/exec/balance_fact.rs) и
//! страховка остатка min(учёт, факт) (core/remain_fact.rs), пыль в режиме «Перегон» (core/dust.rs relay_dust).
//! Данные — сокращённо по фактам живого случая 08.11 (сделка 20261108094918374PIT, mexc → DEX, кошелёк w2; decimals PIT
//! = 9 — по количеству знаков баланса 19259.884524163): зачислено 304051696654.08 PIT; квитанция продажи — одно событие
//! Transfer с кошелька на 291889628787.9168 (= 304051696654.08 × 0.96: налог 4% у токена-рефлексии списан с баланса
//! отправителя БЕЗ события); на кошельке после продажи — 19259.884524163 (доход рефлексии, не наш учёт).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::exec::balance_fact, arb::core::{remain_fact, dust}, core/types.

use std::str::FromStr;

use alloy_primitives::U256;
use chrono::Utc;
use rust_decimal::Decimal;

use arb::core::types::{Asset, Side, TradeRequest, TradeResult, TradeStatus, VenueId};
use arb::venues::bsc_dex::exec::balance_fact::{Seen, judge, rewrite};

/// Десятичное число из строки.
fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

/// Количество PIT в минимальных единицах (decimals 9).
fn u(s: &str) -> U256 {
    let v = d(s) * Decimal::from(1_000_000_000u64);
    U256::from_str(&v.trunc().to_string()).unwrap()
}

/// Блок продажи живого случая: до — зачисленное плюс доход рефлексии, после — только доход рефлексии; события блока
/// с кошелька — одно (291.89 млрд), на кошелёк — нет.
fn pit_block() -> Seen {
    Seen {
        before: u("304051696654.08") + u("19259.884524163"),
        after: u("19259.884524163"),
        block_out: u("291889628787.9168"),
        block_in: U256::ZERO,
    }
}

/// Итог продажи по логам (как exec/result.rs до решения 222): 291.89 млрд по средней цене.
fn pit_sold_by_logs() -> TradeResult {
    TradeResult {
        request: TradeRequest {
            venue: VenueId("bsc_dex".into()),
            asset: Asset {
                ticker: "PIT".into(),
                bsc_contract: None,
            },
            side: Side::Sell,
            qty: d("304051696654.08"),
            worst_price: d("0.0000000001"),
            deal_id: "20261108094918374PIT".into(),
            route: None,
            exit_position: true,
        },
        status: TradeStatus::PartiallyFilled,
        filled_qty: d("291889628787.9168"),
        avg_price: d("0.00000000096"),
        fees: arb::core::types::Fee::single(Decimal::ZERO, "BNB"),
        venue_ref: "0x".into(),
        chain: None,
        finished_at: Utc::now(),
        executor: Some("свой сборщик: прямой".into()),
    }
}

#[test]
fn sell_debit_is_balance_change_not_transfer_log() {
    // Списано с кошелька = всё зачисленное, а не сумма события Transfer.
    let got = judge(Side::Sell, u("291889628787.9168"), &pit_block()).unwrap();
    assert_eq!(got, u("304051696654.08"));
}

#[test]
fn sell_result_rewritten_no_phantom_left() {
    let t = pit_sold_by_logs();
    let usdt = t.filled_qty * t.avg_price;
    let r = rewrite(t, d("304051696654.08"));
    assert_eq!(r.filled_qty, d("304051696654.08"));
    assert_eq!(r.status, TradeStatus::Filled);
    // Выручка USDT не меняется; средняя цена = выручка / списано (как у котировки продажи DEX).
    assert_eq!((r.filled_qty * r.avg_price).round_dp(12), usdt.round_dp(12));
    // Учёт ядра (fills::trade: qty −= filled_qty) — остатка нет; до решения 222 оставалось 12162067866.1632.
    assert_eq!(d("304051696654.08") - r.filled_qty, Decimal::ZERO);
    assert!(r.executor.unwrap().contains("решение 222"));
}

#[test]
fn sell_debit_never_above_request() {
    // Списано больше запрошенного (монеты не этой сделки) — в итог не больше запроса.
    let r = rewrite(pit_sold_by_logs(), d("400000000000"));
    assert_eq!(r.filled_qty, d("304051696654.08"));
}

#[test]
fn other_transfer_in_block_keeps_logs() {
    // В том же блоке поступление той же монеты на кошелёк (или другая наша транзакция) — изменение баланса не только
    // наше: факт остаётся по логам.
    let mut s = pit_block();
    s.block_in = u("1000");
    assert!(judge(Side::Sell, u("291889628787.9168"), &s).is_err());
    let mut s = pit_block();
    s.block_out += u("5");
    assert!(judge(Side::Sell, u("291889628787.9168"), &s).is_err());
}

#[test]
fn buy_credit_is_balance_change() {
    let s = Seen {
        before: u("10"),
        after: u("110"),
        block_out: U256::ZERO,
        block_in: u("100"),
    };
    assert_eq!(judge(Side::Buy, u("100"), &s).unwrap(), u("100"));
    // Баланс не вырос — не та сторона, факт по логам.
    let s = Seen { after: u("5"), ..s };
    assert!(judge(Side::Buy, u("100"), &s).is_err());
}

#[test]
fn remainder_goes_by_min_of_account_and_fact() {
    use arb::core::remain_fact::settle_qty;
    // Живой случай: по учёту 12162067866.1632, на кошельке 19259.884524163 — дальше то, что есть.
    assert_eq!(
        settle_qty(d("12162067866.1632"), d("19259.884524163")),
        d("19259.884524163")
    );
    // Факт больше учёта (доход рефлексии, монеты других сделок) — не больше учёта сделки.
    assert_eq!(settle_qty(d("100"), d("150")), d("100"));
    // На площадке ничего — 0 (сделка закрывается).
    assert_eq!(settle_qty(d("770.395380966"), Decimal::ZERO), Decimal::ZERO);
}

#[test]
fn relay_dust_rule() {
    use arb::core::dust::relay_dust;
    // «Перегон на MEXC (ожидается 0.0039 USDT)» остатка 770 PIT: приход — пыль на MEXC (выручка 0 − газ ≤ 0), продажа
    // здесь не считается — пыль, не гнать.
    assert!(relay_dust(None, true, None));
    assert!(relay_dust(None, true, Some(Decimal::ZERO)));
    // Продажа здесь даёт выручку — продаём здесь, не пыль.
    assert!(!relay_dust(None, true, Some(d("0.5"))));
    // Есть окупаемая цель — перегон, не пыль.
    assert!(!relay_dust(Some(d("0.0039")), true, None));
    // Ни одной оценки «не окупается» (цели не оценены) — не пыль: прежнее поведение.
    assert!(!relay_dust(None, false, None));
}
