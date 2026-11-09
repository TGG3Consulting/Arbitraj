//! Файл: tests/bsc_dex_withdraw_found_gas_236.rs
//! Что делает: решение 236, дополнение п. 7 (техлид 09.11 по поручению Тиграна) — вывод из кошелька DEX, найденный
//! ботом после «исход неизвестен» (resolve_withdraw.rs landed), несёт газ своей квитанции, как обычный путь
//! (wallet/transfer.rs), и газ входит в итог сделки один раз.
//! Квитанция — НАСТОЯЩАЯ квитанция BSC нативного перевода BNB (tests/data/native_2026-10-25/
//! arb_bingx_bnb_withdraw_2026-10-25.json, results[1] — eth_getTransactionReceipt tx 0x8476…1df6: вывод BingX 0,01948 BNB,
//! gasUsed 0x5208, status 0x1). Ядро — настоящая SQLite во временной папке без площадок (как tests/core_deal_gas_236.rs;
//! моков нет).
//! Проверяется: (1) resolve_withdraw::fee — TransferGas в BNB = gasUsed × effectiveGasPrice (сверено с сырыми полями
//! записи); (2) живое ядро: перевод с этой затратой (путь «зачислено по блокчейну» берёт найденный вывод как есть,
//! core/transfer_chain.rs) — fills::transfer: газ не в количестве монет (затрата газа, convert::in_coin), количество =
//! зачислено, в затраты — одна попытка пересчёта (нет живой цены BNB — одна строка «не пересчитана», итог без него).
//! Положительный пересчёт BNB → USDT — живой стакан биржи, в тесте без площадок не проверяется.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{resolve_withdraw, send::receipt}, arb::core::{fills, types}.

mod usdt_return_common;

use std::str::FromStr;

use alloy_primitives::U256;
use arb::core::types::{Asset, CostKind, Transfer, TransferStatus, VenueId};
use arb::venues::bsc_dex::resolve_withdraw;
use arb::venues::bsc_dex::send::receipt::Receipt;
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::Value;
use usdt_return_common::{dir, doge, start};

const REC: &str = "tests/data/native_2026-10-25/arb_bingx_bnb_withdraw_2026-10-25.json";

/// Квитанция перевода из записи и её сырые (gasUsed, effectiveGasPrice). Паника — файла нет.
fn receipt() -> (Receipt, U256, U256) {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(REC).unwrap()).unwrap();
    let r = v["results"][1]["response"]["result"].clone();
    assert_eq!(r["status"], "0x1");
    let hex = |k: &str| U256::from_str(r[k].as_str().unwrap()).unwrap();
    let (used, price) = (hex("gasUsed"), hex("effectiveGasPrice"));
    (serde_json::from_value(r).unwrap(), used, price)
}

/// Перевод в формате ядра (как wallet/transfer.rs transfer).
fn transfer(
    venue: &str,
    amount: &str,
    status: TransferStatus,
    fee: Option<arb::core::types::Cost>,
) -> Transfer {
    Transfer {
        venue: VenueId(venue.into()),
        asset: Asset {
            ticker: "DOGE".into(),
            bsc_contract: None,
        },
        network: "BSC".into(),
        amount: Decimal::from_str(amount).unwrap(),
        status,
        venue_ref: "0x84762200f2bce717c0725ffa926d43f57313c8279d38f2c88189deeaa6391df6".into(),
        tx_hash: Some("0x84762200f2bce717c0725ffa926d43f57313c8279d38f2c88189deeaa6391df6".into()),
        fee,
        updated_at: Utc::now(),
    }
}

#[test]
fn fee_is_gas_of_receipt() {
    let (r, used, price) = receipt();
    assert_eq!(used, U256::from(21000u64));
    let c = resolve_withdraw::fee(&r).unwrap();
    assert_eq!(c.kind, CostKind::TransferGas);
    assert_eq!(c.asset, "BNB");
    let wei = Decimal::from_str(&(used * price).to_string()).unwrap();
    assert_eq!(c.amount, wei / Decimal::from(10u64.pow(18)));
    assert_eq!(
        r.gas_bnb(),
        Some(c.amount),
        "та же формула, что газ квитанции у обычного пути"
    );
}

#[tokio::test]
async fn found_withdraw_gas_charged_once() {
    let (core, _j) = start(&dir("a236wfound"), None).await;
    let (r, _, _) = receipt();
    let fee = resolve_withdraw::fee(&r);
    let gas = fee.as_ref().unwrap().amount;
    let mut x = doge("w7DOGE");
    let spent = x.spent;
    let sent = transfer("bsc_dex", "100", TransferStatus::Pending, fee);
    let credited = transfer("mexc", "99.5", TransferStatus::Credited, None);
    let to = VenueId("mexc".into());
    arb::core::fills::transfer(&core, &mut x, &sent, &credited, &to, Decimal::ONE).await;
    assert_eq!(
        x.qty,
        Decimal::from_str("99.5").unwrap(),
        "газ — не в количестве монет"
    );
    assert_eq!(
        x.spent, spent,
        "нет живой цены BNB — итог без газа, но попытка одна"
    );
    let tries: Vec<_> = x
        .timeline
        .iter()
        .filter(|(_, t)| t.contains("не пересчитана в USDT"))
        .collect();
    assert_eq!(tries.len(), 1, "{:?}", x.timeline);
    assert!(tries[0].1.contains("BNB"), "{:?}", tries);
    assert!(!gas.is_zero());
}
