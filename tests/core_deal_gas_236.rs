//! Файл: tests/core_deal_gas_236.rs
//! Что делает: решение 236, дополнение п. 1 (техлид 09.11) — газ вне итога обмена / вывода идёт в затраты сделки
//! (core/deal_gas.rs). Ядро на настоящей SQLite во временной папке без площадок в реестре (как
//! tests/core_deal_accounting_236.rs; моков нет). Газ — числа живой сделки 20261108142947740PIT (газ двух продаж на
//! DEX из /api/deals: 0.0000265327 и 0.0000254848 BNB).
//! Проверяется: (1) статьи газа из факта транзакции: исполненный обмен — только approve и чаевые (газ самого обмена уже
//! в TradeResult.fees); откат — газ, чаевые, approve; отмена, занявшая nonce, — газ отмены; approve до ошибки без
//! транзакции — из поля ошибки; (2) поле approve_gas_bnb не делает ошибку «ушедшей в сеть» (OpError.sent — только по
//! факту tx: правила повтора решений 110 / 229 не меняются); старая запись без поля читается; (3) живого курса BNB нет
//! (площадок нет) — затраты не меняются, строка в хронологии «не пересчитан», журнал; статьи забираются один раз;
//! (4) charge_closed без сделки — только журнал; у закрытой сделки без курса — итог не меняется, строка в хронологии.
//! Положительный пересчёт BNB → USDT — тем же fills::usdt, что газ обмена (живой стакан биржи) — в тесте без площадок
//! не проверяется.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::gateway, arb::settings, arb::storage, arb::notify, arb::secrets.

mod usdt_return_common;

use std::str::FromStr;

use alloy_primitives::B256;
use arb::core::deal::Step;
use arb::core::deal_gas::{self as dg, Wasted};
use arb::core::op_error::OpError;
use arb::core::types::{ChainTx, CostKind, SendMethod, TxOutcome, VenueId};
use arb::gateway::errors::{ErrorCode, GatewayError};
use rust_decimal::Decimal;
use usdt_return_common::{dir, doge, entries, start};

/// Число из строки (тестовые литералы).
fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

/// Факт транзакции: исход, газ своей квитанции, газ отмены, чаевые, approve.
fn tx(
    outcome: TxOutcome,
    gas: Option<&str>,
    cancel: Option<&str>,
    tip: Option<&str>,
    approve: Option<&str>,
) -> ChainTx {
    ChainTx {
        hash: B256::ZERO,
        nonce: 7,
        channels: vec![],
        method: SendMethod::Single,
        outcome,
        block: None,
        gas_used: None,
        gas_bnb: gas.map(d),
        tip_bnb: tip.map(d),
        cancel_hash: cancel.map(|_| B256::repeat_byte(1)),
        cancel_gas_bnb: cancel.map(d),
        approve_gas_bnb: approve.map(d),
    }
}

/// Статьи как (что, вид, BNB) — для сравнения.
fn view(w: &[Wasted]) -> Vec<(String, CostKind, Decimal)> {
    w.iter().map(|x| (x.what.clone(), x.kind, x.bnb)).collect()
}

#[test]
fn items_from_tx_facts() {
    // Откат обмена: газ квитанции, чаевые (адаптер ставит их при откате в блоке), approve перед обменом.
    let r = tx(
        TxOutcome::Reverted,
        Some("0.0000265327"),
        None,
        Some("0.00001"),
        Some("0.0000254848"),
    );
    assert_eq!(
        view(&dg::of_failed_tx(&r, "обмена", CostKind::Gas)),
        vec![
            ("откат обмена".into(), CostKind::Gas, d("0.0000265327")),
            ("чаевые канала".into(), CostKind::ChannelTip, d("0.00001")),
            ("approve".into(), CostKind::ApproveGas, d("0.0000254848")),
        ]
    );
    // Перевод из кошелька DEX не попал, nonce занят нашей отменой: газа своей квитанции нет, газ отмены есть.
    let c = tx(
        TxOutcome::NotIncluded,
        None,
        Some("0.0000254848"),
        None,
        None,
    );
    assert_eq!(
        view(&dg::of_failed_tx(&c, "перевода", CostKind::TransferGas)),
        vec![(
            "отмена перевода".into(),
            CostKind::TransferGas,
            d("0.0000254848")
        )]
    );
    // Нули не дают статей.
    let z = tx(TxOutcome::Included, Some("0"), None, Some("0"), None);
    assert!(dg::of_failed_tx(&z, "обмена", CostKind::Gas).is_empty());
    // approve до ошибки без транзакции обмена (цена ушла за время блоков approve).
    let e = GatewayError::new(ErrorCode::QuoteMismatch, None, "после approve")
        .with_approve_gas(Some(d("0.0000265327")));
    assert_eq!(
        view(&dg::of_err_approve(&e)),
        vec![("approve".into(), CostKind::ApproveGas, d("0.0000265327"))]
    );
}

#[test]
fn approve_field_keeps_retry_semantics_and_old_records() {
    // Поле газа approve — не факт транзакции обмена: ошибка до отправки обмена остаётся «не ушла в сеть».
    let e = GatewayError::new(ErrorCode::QuoteMismatch, None, "после approve")
        .with_approve_gas(Some(d("0.0000265327")));
    let op = OpError::from(&e);
    assert!(!op.sent && op.retry_safe());
    // JSON туда-обратно; запись без поля (старая) — None.
    let back: GatewayError = serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
    assert_eq!(back.approve_gas(), Some(d("0.0000265327")));
    let mut v = serde_json::to_value(tx(TxOutcome::Included, None, None, None, None)).unwrap();
    assert!(v.get("approve_gas_bnb").is_none(), "None не пишется");
    v.as_object_mut().unwrap().remove("approve_gas_bnb");
    let old: ChainTx = serde_json::from_value(v).unwrap();
    assert_eq!(old.approve_gas_bnb, None);
    // None не затирает уже стоящий газ.
    assert_eq!(
        e.with_approve_gas(None).approve_gas(),
        Some(d("0.0000265327"))
    );
}

#[tokio::test]
async fn no_bnb_price_keeps_spent_and_says_so_once() {
    let (core, j) = start(&dir("a236gas"), None).await;
    let mut x = doge("g1DOGE");
    let spent = x.spent;
    let mut list = dg::of_failed_tx(
        &tx(TxOutcome::Reverted, Some("0.0000265327"), None, None, None),
        "обмена",
        CostKind::Gas,
    );
    dg::charge(&core, &mut x, &VenueId("mexc".into()), &mut list).await;
    assert!(list.is_empty(), "статьи забраны — второй раз не войдут");
    assert_eq!(x.spent, spent, "нет живой цены BNB — итог без газа");
    assert!(
        x.timeline.iter().any(
            |(_, t)| t.contains("откат обмена 0.0000265327 BNB") && t.contains("не пересчитан")
        ),
        "{:?}",
        x.timeline
    );
    assert_eq!(
        entries(
            &j,
            "core.ledger",
            "g1DOGE: газ 0.0000265327 BNB не переведён"
        )
        .await
        .len(),
        1
    );
    // Пустой список — ничего не пишется.
    let n = x.timeline.len();
    dg::charge(&core, &mut x, &VenueId("mexc".into()), &mut list).await;
    assert_eq!(x.timeline.len(), n);
}

#[tokio::test]
async fn closed_without_deal_or_price() {
    let (core, j) = start(&dir("a236gasc"), None).await;
    let v = VenueId("mexc".into());
    let mut list = vec![Wasted {
        what: "отмена перевода".into(),
        kind: CostKind::TransferGas,
        bnb: d("0.0000254848"),
    }];
    dg::charge_closed(&core, "nosuch", &v, &mut list).await;
    assert!(list.is_empty());
    assert_eq!(
        entries(&j, "core.ledger", "перегон вне сделки").await.len(),
        1
    );
    // Закрытая сделка, курса нет — итог прежний, строка в хронологии.
    let mut x = doge("g2DOGE");
    arb::core::ledger::finish(&core, &mut x, Step::Closed, "продано (выгодно)").await;
    let before = core.deals().into_iter().find(|y| y.id == "g2DOGE").unwrap();
    let mut list = vec![Wasted {
        what: "откат перевода".into(),
        kind: CostKind::TransferGas,
        bnb: d("0.0000265327"),
    }];
    dg::charge_closed(&core, "g2DOGE", &v, &mut list).await;
    let after = core.deals().into_iter().find(|y| y.id == "g2DOGE").unwrap();
    assert_eq!(after.result_usd, before.result_usd);
    assert_eq!(after.spent, before.spent);
    assert!(
        after
            .timeline
            .iter()
            .any(|(_, t)| t.contains("откат перевода 0.0000265327 BNB")),
        "{:?}",
        after.timeline
    );
}
