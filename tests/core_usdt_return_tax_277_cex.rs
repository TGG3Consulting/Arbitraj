//! Файл: tests/core_usdt_return_tax_277_cex.rs
//! Что делает: решение 277 (ревью) — замер налога USDT действует только для получателя-DEX. Для получателя-биржи
//! (возврат USDT биржа ↔ биржа: события биржи приходят без хеша и сопоставляются только по сумме) налог перевода USDT —
//! как до решения 277: cex_cex.untaxed_ok не «да» — ошибка UNMEASURED и только опрос; «да» — 0. Замер USDT (в том числе
//! ненулевой) на получателя-биржу не влияет: итог тот же, что без замера.
//! Данные — tests/usdt277_common (записи BingX config_getall и ответа узла на пробу переводов USDT); ненулевой налог —
//! подставлен в записанную проверку (у реального USDT замер даёт 0), чтобы отличить «замер» от «0 по настройке».
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, tests/usdt277_common.

mod usdt277_common;

use rust_decimal::Decimal;
use serde_json::json;

use arb::core::Core;
use arb::core::{credit_expect, transfer_tax};
use usdt277_common::*;

/// Налог перевода USDT и ожидание зачисления возврата bingx → `to` (у mexc включены события баланса).
/// Возвращает: (налог, зарегистрировано ли ожидание).
async fn both(core: &Core, to: &str) -> (Result<Decimal, String>, bool) {
    let (_, net) = bingx_usdt();
    let qty = Decimal::new(100, 0);
    let pct = transfer_tax::pct(core, &v(to), &usdt());
    let t =
        credit_expect::ticket(core, "d1", &v("bingx"), &v(to), &usdt(), &net.network, qty).await;
    (pct, t.is_some())
}

#[tokio::test]
async fn cex_receiver_unchanged_without_untaxed_ok() {
    let core = start("cex-off").await;
    set(&core, Some("mexc"), "venue.balance_events", json!(true)).await;
    let before = both(&core, "mexc").await;
    let mut t = usdt_taxes();
    t.transfer_pct = Decimal::new(5, 1);
    apply(&core, usdt_check_with(t));
    let after = both(&core, "mexc").await;
    let e = after.0.clone().unwrap_err();
    assert!(e.contains(transfer_tax::UNMEASURED), "{e}");
    assert_eq!(before, after, "замер USDT получателю-бирже не виден");
    assert!(!after.1, "только опрос, как до решения 277");
    // Получателю-DEX тот же замер виден.
    assert_eq!(
        transfer_tax::pct(&core, &v("bsc_dex"), &usdt()),
        Ok(Decimal::new(5, 1))
    );
}

#[tokio::test]
async fn cex_receiver_untaxed_ok_gives_zero_not_measured() {
    let core = start("cex-on").await;
    set(&core, Some("mexc"), "venue.balance_events", json!(true)).await;
    set(&core, None, "cex_cex.untaxed_ok", json!(true)).await;
    let before = both(&core, "mexc").await;
    let mut t = usdt_taxes();
    t.transfer_pct = Decimal::new(5, 1);
    apply(&core, usdt_check_with(t));
    let after = both(&core, "mexc").await;
    assert_eq!(
        after.0,
        Ok(Decimal::ZERO),
        "0 по cex_cex.untaxed_ok, не замер 0.5"
    );
    assert_eq!(before, after);
    assert!(
        after.1,
        "как до решения 277: untaxed_ok = да — ожидание есть"
    );
}
