//! Файл: tests/core_usdt_return_tax_277_hash.rs
//! Что делает: решение 277 — возврат USDT биржа → кошелёк DEX с измеренным налогом USDT: событие поступления ERC-20 с
//! хешем, как у реального DEX (Transfer в блоке), сопоставляется с ожиданием по решению 76 (возврат USDT — send_as,
//! зачисления по блокчейну, решения 151, у него нет). Живой бот 14.11.2026: у всех шести возвратов событие DEX пришло
//! раньше хеша от биржи, и хеш события совпал с хешем вывода биржи. События — настоящие переводы USDT блока 129702236
//! (tests/data/receipts_2026-10-25/responses.json, как tests/core_credit_chain.rs): 58,69 USDT (0x3263…0835) — «наш»
//! приход; другой перевод USDT того же блока — «чужой хеш» и «сумма вне допуска».
//! - событие до хеша, затем set_tx с тем же хешем — closing закрывает возврат; событие после set_tx — тоже;
//! - чужой хеш на ту же сумму — снят при set_tx (вызывающий пишет его во «внешнее»), возврат ждёт своего события;
//! - сумма вне допуска — к возврату не прикреплена.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, tests/usdt277_common, tests/receipts_common.

mod receipts_common;
mod usdt277_common;

use alloy_primitives::{Address, U256};
use chrono::Utc;
use rust_decimal::Decimal;

use arb::core::Core;
use arb::core::credit_watch::Ticket;
use arb::core::types::{BalanceChange, BalanceKind, DexAccountId};
use arb::core::{credit_event, credit_expect};
use receipts_common::{load, result};
use usdt277_common::*;

const OUR: &str = "0x3263576c6e5102aa5d25fcfbf93717423687652f2a6dbd04d205523318f20835";
const TRANSFER: &str = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";
/// Кошелёк DEX возврата (id счёта из маппинга биржи-отправителя).
const WALLET: &str = "bingx";

/// Переводы USDT блока 129702236 из записи: (хеш, сумма при 18 знаках).
fn usdt_transfers() -> Vec<(String, Decimal)> {
    let receipts = result(&load("responses.json"), "receipts 129702236");
    let mut out = Vec::new();
    for r in receipts.as_array().unwrap() {
        for l in r["logs"].as_array().unwrap() {
            let topics = l["topics"].as_array().unwrap();
            let token: Address = l["address"].as_str().unwrap().parse().unwrap();
            if topics.len() != 3 || topics[0].as_str() != Some(TRANSFER) || token != USDT {
                continue;
            }
            let hex = l["data"].as_str().unwrap().trim_start_matches("0x");
            let units: u128 = U256::from_str_radix(hex, 16).unwrap().try_into().unwrap();
            let amount = Decimal::from_i128_with_scale(units as i128, 18);
            out.push((r["transactionHash"].as_str().unwrap().to_string(), amount));
        }
    }
    out
}

/// «Наш» приход и другой перевод USDT блока (другой хеш, другая сумма).
fn ours_and_other() -> ((String, Decimal), (String, Decimal)) {
    let all = usdt_transfers();
    let ours = all.iter().find(|(h, _)| h == OUR).unwrap().clone();
    let other = all
        .iter()
        .find(|(h, a)| h != OUR && *a != ours.1)
        .unwrap()
        .clone();
    (ours, other)
}

/// Поступление USDT на кошелёк DEX с хешем, как у адаптера DEX.
fn deposit(hash: &str, delta: Decimal) -> BalanceChange {
    BalanceChange {
        venue: v("bsc_dex"),
        asset: usdt(),
        delta,
        kind: BalanceKind::Deposit,
        own: false,
        tx_hash: Some(hash.to_string()),
        at: Utc::now(),
        source: "Transfer в блоке 129702236".into(),
        account: Some(DexAccountId(WALLET.into())),
    }
}

/// Ядро с измеренным налогом USDT и ожидание возврата bingx → bsc_dex, у которого ожидаемый приход = `arrive`
/// (вывод = приход + комиссия вывода BingX из записи; кошелёк задан, как transfers.rs go).
async fn armed(name: &str, arrive: Decimal) -> (Core, Ticket) {
    let core = start(name).await;
    apply(&core, usdt_check());
    let (_, net) = bingx_usdt();
    let qty = arrive + net.fee_for(arrive).unwrap();
    let t = credit_expect::ticket(
        &core,
        "d1",
        &v("bingx"),
        &v("bsc_dex"),
        &usdt(),
        &net.network,
        qty,
    )
    .await
    .expect("ожидание зачисления");
    t.set_account(Some(DexAccountId(WALLET.into())));
    (core, t)
}

#[tokio::test]
async fn hash_event_before_and_after_set_tx_closes() {
    let ((h, a), _) = ours_and_other();
    let (core, t) = armed("hash-before", a).await;
    credit_event::on_change(&core, deposit(&h, a)).await;
    assert!(
        t.closing().is_none(),
        "хеша от биржи ещё нет — событие только будит опрос"
    );
    assert!(t.set_tx(&h).is_empty(), "тот же хеш — не чужое");
    let c = t.closing().expect("закрыто событием с хешем вывода");
    assert_eq!(c.tx_hash.as_deref(), Some(h.as_str()));
    assert_eq!(c.delta, a);
    drop(t);
    let (core, t) = armed("hash-after", a).await;
    assert!(t.set_tx(&h).is_empty());
    credit_event::on_change(&core, deposit(&h, a)).await;
    assert!(t.closing().is_some(), "событие после хеша — закрывает");
}

#[tokio::test]
async fn foreign_hash_split_off_return_keeps_waiting() {
    let ((h, a), (other, _)) = ours_and_other();
    let (core, t) = armed("hash-foreign", a).await;
    // Чужой перевод на ту же сумму до хеша от биржи — прикреплён (хеша ещё нет)…
    credit_event::on_change(&core, deposit(&other, a)).await;
    // …и снят при set_tx: вызывающий (transfers.rs) пишет его во «внешнее изменение баланса».
    let foreign = t.set_tx(&h);
    assert_eq!(foreign.len(), 1);
    assert_eq!(foreign[0].tx_hash.as_deref(), Some(other.as_str()));
    assert!(
        !t.has_event() && t.closing().is_none(),
        "возврат ждёт своего события"
    );
    credit_event::on_change(&core, deposit(&h, a)).await;
    assert!(t.closing().is_some(), "своё событие закрывает");
}

#[tokio::test]
async fn amount_outside_tolerance_not_attached() {
    let ((h, a), (_, other_amount)) = ours_and_other();
    let (core, t) = armed("hash-amount", a).await;
    assert!(t.set_tx(&h).is_empty());
    credit_event::on_change(&core, deposit(&h, other_amount)).await;
    assert!(
        !t.has_event() && t.closing().is_none(),
        "сумма вне допуска (0) — не наше"
    );
}
