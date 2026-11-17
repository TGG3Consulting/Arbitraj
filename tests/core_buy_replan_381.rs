//! Файл: tests/core_buy_replan_381.rs
//! Что делает: решение 381 — пересчёт плана после срыва покупки DEX до отправки (core/buy_replan.rs):
//! (1) какие ошибки покупки допускают новый план: «цена ушла» до отправки (QuoteMismatch без транзакции в сети) и
//!     «не попала в блок, nonce занят нашей отменой в блоке» (решение 229) — да; та же цена с транзакцией в сети, откат
//!     в сети, исход неизвестен, устаревшие данные, нет маршрута, отказ канала, «не попала» без подтверждённой отмены —
//!     нет;
//! (2) вердикт расчёта → план: прибыль выше порогов и списание по худшей цене не больше предела — план; списание больше
//!     предела — отказ с числами; отказ расчёта — его причина. Расчёт — настоящий Calc ядра на записанных данных CAKE
//!     (tests/size_common/calc.rs: asks BingX 2026-10-23, пул CAKE/USDT V2 на блоке 0x7b936ae).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{buy_replan, op_error, profit}, tests/size_common.

mod size_common;

use arb::core::buy_replan::{accept, refused_note, replannable};
use arb::core::op_error::OpError;
use arb::core::profit::Verdict;
use arb::gateway::errors::ErrorCode;
use size_common::{Rec, d};

/// Ошибка покупки с кодом, признаком «ушла в сеть» и «отмена в блоке».
fn err(code: Option<ErrorCode>, sent: bool, cancelled: bool) -> OpError {
    OpError {
        code,
        message: "покупка".into(),
        sent,
        cancelled,
    }
}

#[test]
fn replannable_only_unsent_price_moves_and_confirmed_cancel() {
    // «Цена в пуле сейчас … хуже допустимой» / «свежая средняя цена дороже границы сделки» — до отправки.
    assert!(replannable(&err(
        Some(ErrorCode::QuoteMismatch),
        false,
        false
    )));
    // Решение 229: не попала в блок, nonce занят нашей отменой в блоке — исходная уже не исполнится.
    assert!(replannable(&err(
        Some(ErrorCode::TxNotIncluded),
        true,
        true
    )));
    // Транзакция в сети — нового плана нет (стартовое правило Тиграна).
    assert!(!replannable(&err(
        Some(ErrorCode::QuoteMismatch),
        true,
        false
    )));
    assert!(!replannable(&err(
        Some(ErrorCode::SwapReverted),
        true,
        false
    )));
    assert!(!replannable(&err(
        Some(ErrorCode::OutcomeUnknown),
        true,
        false
    )));
    assert!(!replannable(&err(
        Some(ErrorCode::TxNotIncluded),
        true,
        false
    )));
    // Прочие отказы до отправки — не про цену.
    assert!(!replannable(&err(Some(ErrorCode::StaleData), false, false)));
    assert!(!replannable(&err(Some(ErrorCode::NoRoute), false, false)));
    assert!(!replannable(&err(
        Some(ErrorCode::ChannelRejected),
        false,
        false
    )));
    assert!(!replannable(&err(None, false, false)));
}

#[tokio::test]
async fn accept_keeps_cap_and_reasons() {
    let rec = Rec::load("buy-replan-381").await;
    let c = size_common::calc::calc(&rec, d("10"));
    let spend = c.spend_max;
    // Предел ровно на списание — план.
    let ok = accept(Verdict::Accept(c.clone()), spend).expect("списание в пределе");
    assert_eq!(ok.qty, c.qty);
    // Предел меньше списания — отказ с числами.
    let cap = spend - d("0.0001");
    let e = accept(Verdict::Accept(c.clone()), cap).unwrap_err();
    assert!(e.contains("больше предела списания"), "{e}");
    assert!(e.contains(&cap.to_string()), "{e}");
    // Отказ расчёта — его причина, без изменений.
    let why = "прибыль 0.1 ниже порога money.min_profit_usd 0.5".to_string();
    let r = accept(
        Verdict::Reject {
            reason: why.clone(),
            best: Some(c),
        },
        spend,
    );
    assert_eq!(r.unwrap_err(), why);
    assert!(refused_note(1, &why).contains(&why));
}
