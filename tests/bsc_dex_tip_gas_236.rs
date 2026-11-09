//! Файл: tests/bsc_dex_tip_gas_236.rs
//! Что делает: решение 236, дополнение п. 6 (техлид 09.11 по поручению Тиграна) — газ отдельной транзакции перевода
//! чаевых канала (exec/txs.rs: перевод BNB с nonce + 1 в том же бандле) идёт в затраты сделки, как газ approve.
//! Квитанция — НАСТОЯЩАЯ квитанция BSC простого перевода BNB (tests/data/receipts_2026-10-25/responses.json,
//! eth_getBlockReceipts блока 129702236 = 0x7bb195c, results[2].result[44], tx 0xb306…8b42: gasUsed 0x5208 = 21000,
//! effectiveGasPrice 0x2faf081 = 50000001 wei, логов нет) — тот же вид транзакции, что перевод чаевых (BNB на адрес,
//! пустые данные). Ядро — настоящая SQLite во временной папке без площадок (как tests/core_deal_gas_236.rs; моков нет).
//! Проверяется: (1) exec/chain_tx.rs set_tip_gas — газ по квитанции = gasUsed × effectiveGasPrice; квитанции нет —
//! текст для журнала, газ не ставится; чаевые не в блоке (tip_bnb None) или хеша перевода нет — квитанция не нужна;
//! (2) core/deal_gas.rs — статья «газ перевода чаевых» у исполненного обмена и у неудачной транзакции, один раз;
//! (3) старая запись ChainTx без новых полей читается, None не пишется; (4) живое ядро: статья доходит до хронологии
//! сделки (нет живой цены BNB — «не пересчитан», итог без него), статьи забираются.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{exec::chain_tx, send::receipt}, arb::core::{deal_gas, types}.

mod usdt_return_common;

use std::str::FromStr;

use alloy_primitives::B256;
use arb::core::deal_gas as dg;
use arb::core::types::{
    Asset, ChainTx, CostKind, SendMethod, Side, TradeRequest, TradeResult, TradeStatus, TxOutcome,
    VenueId,
};
use arb::venues::bsc_dex::exec::chain_tx::set_tip_gas;
use arb::venues::bsc_dex::send::receipt::Receipt;
use rust_decimal::Decimal;
use serde_json::Value;
use usdt_return_common::{dir, doge, start};

const RECEIPTS: &str = "tests/data/receipts_2026-10-25/responses.json";

/// Число из строки (тестовые литералы).
fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

/// Квитанция простого перевода BNB tx 0xb306… из записи (results[2].result[44]). Паника — файла нет.
fn plain_transfer() -> (B256, Receipt) {
    let rec: Value = serde_json::from_str(&std::fs::read_to_string(RECEIPTS).unwrap()).unwrap();
    let r = rec["results"][2]["response"]["result"][44].clone();
    assert_eq!(r["gasUsed"], "0x5208");
    assert_eq!(
        r["logs"],
        serde_json::json!([]),
        "простой перевод BNB — без логов"
    );
    let hash = B256::from_str(r["transactionHash"].as_str().unwrap()).unwrap();
    (hash, serde_json::from_value(r).unwrap())
}

/// Факт обмена: исход, газ своей квитанции, чаевые, хеш перевода чаевых.
fn swap(outcome: TxOutcome, tip: Option<&str>, tip_hash: Option<B256>) -> ChainTx {
    ChainTx {
        hash: B256::repeat_byte(7),
        nonce: 7,
        channels: vec!["blockrazor_builder".into()],
        method: SendMethod::Bundle,
        outcome,
        block: None,
        gas_used: None,
        gas_bnb: Some(d("0.0000265327")),
        tip_bnb: tip.map(d),
        cancel_hash: None,
        cancel_gas_bnb: None,
        approve_gas_bnb: None,
        tip_hash,
        tip_gas_bnb: None,
    }
}

#[test]
fn tip_gas_from_real_receipt() {
    let (h, r) = plain_transfer();
    // 21000 × 50000001 wei = 1050000021000 wei.
    let want = d("0.000001050000021");
    assert_eq!(r.gas_bnb(), Some(want));
    let mut c = swap(TxOutcome::Included, Some("0.00001"), Some(h));
    assert_eq!(set_tip_gas(&mut c, Some(&r)), None);
    assert_eq!(c.tip_gas_bnb, Some(want));
    // Откат обмена в блоке — чаевые учтены адаптером (chain_tx), газ их перевода тоже.
    let mut c = swap(TxOutcome::Reverted, Some("0.00001"), Some(h));
    assert_eq!(set_tip_gas(&mut c, Some(&r)), None);
    assert_eq!(c.tip_gas_bnb, Some(want));
    // Квитанции нет — газ не ставится, текст для журнала.
    let mut c = swap(TxOutcome::Included, Some("0.00001"), Some(h));
    let why = set_tip_gas(&mut c, None).unwrap();
    assert!(
        why.contains("не прочитан") && why.contains("в итог сделки не вошёл"),
        "{why}"
    );
    assert_eq!(c.tip_gas_bnb, None);
    // Чаевые не в блоке (chain_tx не ставит tip_bnb) или хеша нет (старая запись) — квитанция не нужна.
    let mut c = swap(TxOutcome::NotIncluded, None, Some(h));
    assert_eq!(set_tip_gas(&mut c, Some(&r)), None);
    assert_eq!(c.tip_gas_bnb, None);
    let mut c = swap(TxOutcome::Included, Some("0.00001"), None);
    assert_eq!(set_tip_gas(&mut c, Some(&r)), None);
    assert_eq!(c.tip_gas_bnb, None);
}

#[test]
fn tip_gas_is_a_deal_cost_once() {
    let (h, r) = plain_transfer();
    let mut c = swap(TxOutcome::Reverted, Some("0.00001"), Some(h));
    set_tip_gas(&mut c, Some(&r));
    let items: Vec<(String, CostKind, Decimal)> = dg::of_failed_tx(&c, "обмена", CostKind::Gas)
        .into_iter()
        .map(|w| (w.what, w.kind, w.bnb))
        .collect();
    assert_eq!(
        items,
        vec![
            ("откат обмена".into(), CostKind::Gas, d("0.0000265327")),
            ("чаевые канала".into(), CostKind::ChannelTip, d("0.00001")),
            (
                "газ перевода чаевых".into(),
                CostKind::ChannelTip,
                d("0.000001050000021")
            ),
        ]
    );
    // Старая запись без новых полей читается; None не пишется.
    let mut v = serde_json::to_value(swap(TxOutcome::Included, None, None)).unwrap();
    assert!(v.get("tip_hash").is_none() && v.get("tip_gas_bnb").is_none());
    v.as_object_mut().unwrap().remove("approve_gas_bnb");
    let old: ChainTx = serde_json::from_value(v).unwrap();
    assert_eq!((old.tip_hash, old.tip_gas_bnb), (None, None));
    // Туда-обратно с полями.
    let back: ChainTx = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
    assert_eq!(
        (back.tip_hash, back.tip_gas_bnb),
        (Some(h), Some(d("0.000001050000021")))
    );
}

#[tokio::test]
async fn tip_gas_reaches_deal_timeline() {
    let (core, _j) = start(&dir("a236tipgas"), None).await;
    let (h, r) = plain_transfer();
    let mut c = swap(TxOutcome::Included, Some("0.00001"), Some(h));
    set_tip_gas(&mut c, Some(&r));
    let res = TradeResult {
        status: TradeStatus::Filled,
        filled_qty: Decimal::ONE,
        avg_price: Decimal::ONE,
        fees: vec![],
        venue_ref: c.hash.to_string(),
        chain: Some(c),
        finished_at: chrono::Utc::now(),
        executor: None,
        request: TradeRequest {
            venue: VenueId("bsc_dex".into()),
            asset: Asset {
                ticker: "DOGE".into(),
                bsc_contract: None,
            },
            side: Side::Sell,
            qty: Decimal::ONE,
            worst_price: Decimal::ONE,
            deal_id: "g6DOGE".into(),
            route: None,
            exit_position: false,
            remeasure: None,
        },
    };
    let mut list = dg::of_filled(&res);
    assert_eq!(
        list.len(),
        2,
        "чаевые и газ их перевода (газ обмена — в fees)"
    );
    let mut x = doge("g6DOGE");
    let spent = x.spent;
    dg::charge(&core, &mut x, &VenueId("mexc".into()), &mut list).await;
    assert!(list.is_empty(), "статьи забраны — второй раз не войдут");
    assert_eq!(x.spent, spent, "нет живой цены BNB — итог без газа");
    assert!(
        x.timeline
            .iter()
            .any(|(_, t)| t.contains("газ перевода чаевых 0.000001050000021000 BNB")),
        "{:?}",
        x.timeline
    );
}
