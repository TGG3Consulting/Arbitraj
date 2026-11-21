//! Файл: tests/core_usdt_return_tax_277.rs
//! Что делает: решение 277 — налог перевода USDT при возврате с биржи на кошелёк DEX берётся из замера проверки налогов
//! служебной монеты (venues/bsc_dex/check/service_tax.rs → событие CoinCheck ядру), а не «налог перевода не проверен».
//! Живой бот 14.11.2026: 6 возвратов USDT (bingx ×3, mexc ×3) шли «зачисление USDT только по опросу статуса — USDT:
//! налог перевода не проверен», поступление на кошелёк писалось Warn core.balance_external.
//! - налог USDT не измерен: ожидания нет (как раньше), причина — «налог перевода не проверен»;
//! - налог измерен: ожидание регистрируется, приход = сумма − комиссия вывода, поступление прикрепляется к возврату;
//! - ненулевой налог USDT: ожидаемый приход уменьшен на налог (значение налога подставлено в записанную проверку:
//!   у реального USDT замер даёт 0, записи ненулевого налога USDT нет — проверяется формула credit_expect.rs);
//! - измерено, затем сбой перепроверки: прежний налог сохраняется (service_tax::outcome), ожидание регистрируется;
//! - service_tax::deliver кладёт итог в checks и отправляет событие CoinCheck в канал ядра;
//! - проверка USDT — is_quote, проверка другой монеты — нет.
//!
//! Данные — tests/usdt277_common (записи BingX config_getall и ответа узла на пробу переводов USDT).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::venues::bsc_dex::check::{service_tax, state}, tests/usdt277_common. Моков нет:
//! площадок в шлюзе нет — проверяется решение ядра (комиссия вывода — из справочника сетей ядра, credit_expect.rs).

mod usdt277_common;

use alloy_primitives::address;
use rust_decimal::Decimal;
use tokio::sync::mpsc;

use arb::core::types::{BalanceChange, BalanceKind, CheckState};
use arb::core::{credit_event, credit_expect, transfer_tax};
use arb::gateway::events::MarketEvent;
use arb::venues::bsc_dex::check::service_tax;
use arb::venues::bsc_dex::check::state::Checks;
use usdt277_common::*;

/// Ожидаемый приход возврата bingx → bsc_dex суммы `qty`.
async fn arrival(core: &arb::core::Core, qty: Decimal) -> Result<Decimal, String> {
    let (_, net) = bingx_usdt();
    credit_expect::arrival(core, &v("bingx"), &v("bsc_dex"), &usdt(), &net.network, qty).await
}

/// Ожидание зачисления возврата bingx → bsc_dex суммы `qty`.
async fn ticket(core: &arb::core::Core, qty: Decimal) -> Option<arb::core::credit_watch::Ticket> {
    let (_, net) = bingx_usdt();
    credit_expect::ticket(
        core,
        "d1",
        &v("bingx"),
        &v("bsc_dex"),
        &usdt(),
        &net.network,
        qty,
    )
    .await
}

#[tokio::test]
async fn unmeasured_usdt_tax_keeps_poll_only() {
    let core = start("off").await;
    let qty = Decimal::new(100, 0);
    let e = arrival(&core, qty).await.unwrap_err();
    assert!(e.contains("налог перевода не проверен"), "{e}");
    assert!(
        ticket(&core, qty).await.is_none(),
        "без замера налога — только опрос, как раньше"
    );
}

#[tokio::test]
async fn measured_usdt_tax_registers_and_matches() {
    let core = start("on").await;
    apply(&core, usdt_check());
    let qty = Decimal::new(100, 0);
    let fee = bingx_usdt().1.fee_for(qty).unwrap();
    // Налог USDT измерен живым узлом — 0: приход = сумма − комиссия вывода BingX.
    let got = arrival(&core, qty).await.unwrap();
    assert_eq!(got, qty - fee);
    let t = ticket(&core, qty)
        .await
        .expect("ожидание зачисления зарегистрировано");
    let dep = BalanceChange {
        venue: v("bsc_dex"),
        asset: usdt(),
        delta: got,
        kind: BalanceKind::Deposit,
        own: false,
        tx_hash: None,
        at: chrono::Utc::now(),
        source: "тест".into(),
        account: None,
    };
    credit_event::on_change(&core, dep).await;
    assert!(
        t.has_event(),
        "поступление прикреплено к возврату, не внешнее изменение"
    );
}

#[tokio::test]
async fn nonzero_usdt_tax_reduces_expected() {
    let core = start("tax").await;
    let mut t = usdt_taxes();
    t.transfer_pct = Decimal::new(5, 1);
    let pct = t.transfer_pct;
    apply(&core, usdt_check_with(t));
    let qty = Decimal::new(100, 0);
    let fee = bingx_usdt().1.fee_for(qty).unwrap();
    let got = arrival(&core, qty).await.unwrap();
    assert_eq!(
        got,
        (qty - fee) * (Decimal::ONE - pct / Decimal::ONE_HUNDRED)
    );
    assert!(got < qty - fee, "налог уменьшил ожидаемый приход");
}

#[tokio::test]
async fn recheck_failure_keeps_measured_and_delivers() {
    let core = start("keep").await;
    let checks = Checks::default();
    let (tx, mut rx) = mpsc::channel(4);
    let ok = service_tax::outcome(&checks, v("bsc_dex"), "USDT", USDT, &Ok(usdt_taxes()));
    service_tax::deliver(&checks, &tx, ok.clone()).await;
    // Сбой перепроверки (текст причины — как у сбоя узла; значение не важно, важен Err).
    let err: Result<_, String> = Err("узел: таймаут".into());
    let kept = service_tax::outcome(&checks, v("bsc_dex"), "USDT", USDT, &err);
    assert!(
        matches!(&kept.taxes, CheckState::Known(t) if *t == usdt_taxes()),
        "прежний налог"
    );
    assert_eq!(kept.checked_at, ok.checked_at, "время прежнего замера");
    assert_eq!(kept.wait.as_deref(), Some("узел: таймаут"));
    service_tax::deliver(&checks, &tx, kept.clone()).await;
    // Оба итога ушли в канал ядра; в checks — последний.
    for want in [&ok, &kept] {
        match rx.try_recv() {
            Ok(MarketEvent::CoinCheck(c)) => assert_eq!(c.wait, want.wait),
            other => panic!("ожидалось событие CoinCheck, пришло {:?}", other.is_ok()),
        }
    }
    assert_eq!(checks.coin(&USDT).unwrap().wait, kept.wait);
    apply(&core, kept);
    assert!(
        ticket(&core, Decimal::new(100, 0)).await.is_some(),
        "ожидание по прежнему замеру"
    );
    // Сбой без прошлого замера — «не проверено».
    let none = service_tax::outcome(&Checks::default(), v("bsc_dex"), "USDT", USDT, &err);
    assert!(matches!(none.taxes, CheckState::NotChecked));
}

#[tokio::test]
async fn usdt_check_is_quote_other_coin_not() {
    let core = start("quote").await;
    let c = usdt_check();
    assert!(transfer_tax::is_quote(&core, &c));
    let mut other = c.clone();
    // Другой адрес из той же записи: пул-источник пробы (PancakeSwap V2 WBNB/USDT, tests/bsc_dex_service_tax.rs).
    other.asset.contract = Some(address!("0x16b9a82891338f9ba80e2d6970fdda79d1eb0dae"));
    assert!(!transfer_tax::is_quote(&core, &other));
}
