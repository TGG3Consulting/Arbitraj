//! Файл: tests/core_buy_replan_381.rs
//! Что делает: решение 381 — пересчёт плана после срыва покупки DEX до отправки (core/buy_replan.rs):
//! (1) какие ошибки покупки допускают новый план (ревью t167, Б2 и Н2): только «цена ушла» до отправки —
//!     QuoteMismatch с пометкой адаптера price_moved и без транзакции в сети; QuoteMismatch без пометки (сверка с
//!     квотером, минимум 0, новый approve, «обе хуже»), та же пометка с транзакцией в сети, «не попала в блок» с
//!     подтверждённой отменой и без неё, откат в сети, исход неизвестен, устаревшие данные, нет маршрута, отказ канала —
//!     нет;
//! (1а) замки перед повторной покупкой (ревью t167, Б1): risk::can_open и сухой режим — оба прошли — покупка; любой не
//!     прошёл — закрытие с его причиной (сначала риск, как перед первой покупкой);
//! (2) вердикт расчёта → план: прибыль выше порогов и списание по худшей цене не больше предела — план; списание больше
//!     предела — отказ с числами; отказ расчёта — его причина. Расчёт — настоящий Calc ядра на записанных данных CAKE
//!     (tests/size_common/calc.rs: asks BingX 2026-10-23, пул CAKE/USDT V2 на блоке 0x7b936ae).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{buy_replan, op_error, profit}, tests/size_common.

mod size_common;

use arb::core::buy_replan::{accept, ready, refused_note, replannable};
use arb::core::op_error::OpError;
use arb::core::profit::Verdict;
use arb::gateway::errors::ErrorCode;
use size_common::{Rec, d};

/// Ошибка покупки с кодом, признаками «ушла в сеть», «отмена в блоке» и «цена ушла».
fn err(code: Option<ErrorCode>, sent: bool, cancelled: bool, price_moved: bool) -> OpError {
    OpError {
        code,
        message: "покупка".into(),
        sent,
        cancelled,
        price_moved,
    }
}

#[test]
fn replannable_only_unsent_price_moved() {
    let qm = Some(ErrorCode::QuoteMismatch);
    // «Цена в пуле сейчас … хуже допустимой» / «свежая средняя цена дороже границы сделки» — до отправки.
    assert!(replannable(&err(qm, false, false, true)));
    // QuoteMismatch без пометки «цена ушла» (квотер, минимум 0, новый approve, «обе хуже») — на тех же данных повторится.
    assert!(!replannable(&err(qm, false, false, false)));
    // Транзакция в сети — нового плана нет (стартовое правило Тиграна).
    assert!(!replannable(&err(qm, true, false, true)));
    // Ревью t167, Б2: «не попала в блок, nonce занят нашей отменой» — обмен был отправлен; до решения владельца — нет.
    let tx = Some(ErrorCode::TxNotIncluded);
    assert!(!replannable(&err(tx, true, true, false)));
    assert!(!replannable(&err(tx, true, false, false)));
    for code in [
        ErrorCode::SwapReverted,
        ErrorCode::OutcomeUnknown,
        ErrorCode::StaleData,
        ErrorCode::NoRoute,
        ErrorCode::ChannelRejected,
    ] {
        assert!(
            !replannable(&err(Some(code), false, false, true)),
            "{code:?}"
        );
    }
    assert!(!replannable(&err(None, false, false, true)));
}

#[test]
fn ready_checks_risk_then_dry_mode() {
    assert_eq!(ready(Ok(()), Ok(())), Ok(()));
    let paused = "площадка bsc_dex на паузе: ошибок подряд 3".to_string();
    let dry = "остановлена: включён сухой режим до покупки — ордер не ставился".to_string();
    let e = ready(Err(paused.clone()), Err(dry.clone())).unwrap_err();
    assert!(e.contains(&paused) && !e.contains(&dry), "{e}");
    let e = ready(Ok(()), Err(dry.clone())).unwrap_err();
    assert!(e.contains(&dry), "{e}");
    let e = ready(Err(paused.clone()), Ok(())).unwrap_err();
    assert!(
        e.starts_with("перед покупкой по пересчитанному плану"),
        "{e}"
    );
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
