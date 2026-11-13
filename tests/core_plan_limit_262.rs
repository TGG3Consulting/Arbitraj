//! Файл: tests/core_plan_limit_262.rs
//! Что делает: решение 262 (Тигран 13.11, ТЗ «Покупка на A»: «лимитный ордер IOC … с ценой не выше худшего уровня из
//! плана») — лимит ордера покупки на бирже берётся из плана, а не из стакана в момент ордера (моков логики нет):
//! (план — tests/core_plan_worst_262.rs). Ордер: живое ядро с настоящим адаптером XT.com (как tests/core_exec_book_238.rs: локальный сервер с записанными
//!   ответами — справочник пар GET /v4/public/symbol 06.11, стакан REST — пример официальной документации, в нём asks
//!   пусто). Числа живого случая 20261112213305466BTW (данные бота 12.11): объём плана 21.9377, худший уровень плана
//!   0.9552, spend_max 20.95489104 (spend_fee 0). Покупка с худшим уровнем плана: лимит попытки — 0.9552 (источник —
//!   план), стакан не запрашивается ни разу, бюджет проверяется по 0.9552; ордер доходит до адаптера (здесь — отказ до
//!   отправки: не задана venue.order_time_in_force). Свежий стакан той же пары худшую цену покупки не дал бы вовсе
//!   (asks пусто) — прежний путь (worst_order) отказал бы.
//!   Стакана BTW с ask 1.48 в записанных данных нет — его роль «свежий стакан, не совпадающий с планом» играет
//!   записанный стакан XT.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core (trade_ops, worst), arb::venues::xt, tests/xt_plan_common (сервер XT и ядро).

mod xt_plan_common;

use std::str::FromStr;
use std::sync::atomic::Ordering;

use arb::core::Core;
use arb::core::types::{Asset, Side, VenueId};
use rust_decimal::Decimal;
use xt_plan_common::start;

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn btc() -> Asset {
    Asset {
        ticker: "BTC".into(),
        bsc_contract: None,
    }
}

/// Покупка BTC на xt через trade_seen. Принимает: ядро, худший уровень плана, бюджет. Возвращает: ошибку и Worst попытки.
async fn buy(
    core: &Core,
    plan: Option<Decimal>,
    cap: Decimal,
) -> (String, Option<arb::core::worst::Worst>) {
    let mut seen = None;
    let (v, qty) = (VenueId("xt".into()), dec("21.9377"));
    let r = arb::core::trade_ops::trade_seen(
        core,
        "20261113000000000BTC",
        &v,
        &btc(),
        Side::Buy,
        qty,
        None,
        dec("0.9552"),
        plan,
        Some(cap),
        None,
        &mut seen,
        None,
        None,
        false,
    )
    .await;
    (r.unwrap_err().message, seen)
}

#[tokio::test]
async fn cex_buy_limit_is_plan_worst_not_order_time_book() {
    let (core, depth) = start("buy").await;
    let (plan, spend_max) = (dec("0.9552"), dec("20.95489104"));
    // Бюджет ровно spend_max плана: qty × 0.9552 = 20.95489104 — проходит, ордер дошёл до адаптера.
    let (e, seen) = buy(&core, Some(plan), spend_max).await;
    let w = seen.expect("лимит посчитан");
    assert_eq!(w.price, plan, "лимит = худший уровень плана");
    assert!(w.plan && !w.rest && w.book_age_ms.is_none());
    assert!(e.contains("venue.order_time_in_force"), "{e}");
    assert_eq!(
        depth.load(Ordering::SeqCst),
        0,
        "стакан в момент ордера не берётся"
    );
    // Бюджет меньше — отказ по худшей цене ИЗ ПЛАНА.
    let (e, _) = buy(&core, Some(plan), dec("20.95")).await;
    assert!(e.contains("по худшей цене 0.9552"), "{e}");
    // Без худшего уровня плана покупка на бирже не ставится (свежий стакан не подставляется).
    let (e, seen) = buy(&core, None, spend_max).await;
    assert!(e.contains("нет худшего уровня плана"), "{e}");
    assert!(seen.is_none());
    assert_eq!(depth.load(Ordering::SeqCst), 0);
    // Прежний путь (стакан в момент ордера) на том же записанном стакане цены покупки не даёт: asks пусто.
    let v = VenueId("xt".into());
    let old = arb::core::worst::worst_order(&core, &v, &btc(), Side::Buy, dec("21.9377"), plan)
        .await
        .unwrap_err();
    assert!(old.message.contains("не хватает"), "{}", old.message);
    assert_eq!(depth.load(Ordering::SeqCst), 1);
}
