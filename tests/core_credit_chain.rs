//! Файл: tests/core_credit_chain.rs
//! Что делает: решение 151 — зачисление перевода сделки на кошелёк DEX по приходу в блокчейне, без хеша от
//! биржи-отправителя, в настоящем ядре на SQLite: core/credit_watch.rs (ожидание), core/credit_chain.rs (Ticket::set_chain,
//! chain_hit, проверка), core/credit_event.rs (сверка события). События — настоящие переводы USDT из записи квитанций
//! блока 129702236 (tests/data/receipts_2026-10-25/responses.json): 58,69 USDT (0x3263…0835) — «наш» приход; первый
//! перевод USDT блока (другая сумма) — «чужое количество»; токен первого не-USDT перевода блока — «чужой токен».
//! - приход нужного количества на кошелёк сделки — зачислено без хеша биржи;
//! - чужое количество, чужой токен, другой кошелёк, приход раньше вывода, выключатель «нет» — не зачислено;
//! - второй ожидаемый приход того же токена на тот же кошелёк — не зачислено.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::settings, arb::storage, tests/receipts_common. Моков нет: площадок в реестре нет —
//! проверяется решение ядра по событию («вооружение» по виду площадки — transfers.rs, здесь — Ticket::set_chain).

mod receipts_common;

use alloy_primitives::{Address, U256};
use chrono::{Duration, Utc};
use rust_decimal::Decimal;
use serde_json::json;
use tokio::sync::broadcast;

use arb::core::Core;
use arb::core::deps::Deps;
use arb::core::types::{Asset, BalanceChange, BalanceKind, DexAccountId, VenueId};
use arb::core::{credit_event, credit_watch, transfer_credit};
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use receipts_common::{load, result};

const OUR: &str = "0x3263576c6e5102aa5d25fcfbf93717423687652f2a6dbd04d205523318f20835";
const USDT: &str = "0x55d398326f99059ff775485246999027b3197955";
const TRANSFER: &str = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";
/// Кошелёк сделки (id счёта DEX из маппинга биржи-отправителя).
const WALLET: &str = "bingx";

/// Ядро на базе во временной папке и хранилище настроек.
async fn start(name: &str) -> (Core, Store) {
    let d = std::env::temp_dir().join(format!("arb-chain-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    let notifier = Notifier {
        http: reqwest::Client::new(),
        settings: settings.clone(),
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        journal: journal.clone(),
    };
    let core = Core::start(Deps {
        gw: Registry::new(),
        settings: settings.clone(),
        journal,
        notifier,
        db,
        changes,
        buffer: 64,
        data_dir: d.to_path_buf(),
    })
    .await;
    (core, settings)
}

/// Настройки у DEX: допуск 0, закрытие по событию (решение 76) и зачисление по блокчейну (решение 151).
async fn configure(s: &Store, chain: bool) {
    let v = Scope::Venue("bsc_dex".into());
    let set = [
        ("transfer.credit_amount_tolerance_pct", json!("0")),
        ("transfer.credit_by_event", json!(true)),
        ("transfer.credit_by_chain", json!(chain)),
    ];
    for (name, value) in set {
        s.set(name, v.clone(), Some(value), "тест").await.unwrap();
    }
}

/// Переводы токенов блока 129702236 из записи: (хеш, токен, сумма при 18 знаках).
fn transfers() -> Vec<(String, Address, Decimal)> {
    let receipts = result(&load("responses.json"), "receipts 129702236");
    let mut out = Vec::new();
    for r in receipts.as_array().unwrap() {
        let hash = r["transactionHash"].as_str().unwrap();
        for l in r["logs"].as_array().unwrap() {
            let topics = l["topics"].as_array().unwrap();
            if topics.len() != 3 || topics[0].as_str() != Some(TRANSFER) {
                continue;
            }
            let raw =
                U256::from_str_radix(l["data"].as_str().unwrap().trim_start_matches("0x"), 16);
            let Ok(raw) = raw else { continue };
            let units: u128 = raw.try_into().unwrap();
            let amount = Decimal::from_i128_with_scale(units as i128, 18);
            out.push((
                hash.to_string(),
                l["address"].as_str().unwrap().parse().unwrap(),
                amount,
            ));
        }
    }
    out
}

fn asset(token: Address) -> Asset {
    Asset {
        ticker: "USDT".into(),
        bsc_contract: Some(token),
    }
}

fn usdt() -> Address {
    USDT.parse().unwrap()
}

/// Поступление на кошелёк. Принимает: хеш, токен, сумму, кошелёк.
fn deposit(hash: &str, token: Address, delta: Decimal, wallet: &str) -> BalanceChange {
    BalanceChange {
        venue: VenueId("bsc_dex".into()),
        asset: asset(token),
        delta,
        kind: BalanceKind::Deposit,
        own: false,
        tx_hash: Some(hash.to_string()),
        at: Utc::now(),
        source: "Transfer в блоке 129702236".into(),
        account: Some(DexAccountId(wallet.into())),
    }
}

/// Ожидание перевода сделки bingx → DEX, «вооружённое» с момента `since`.
fn ticket(
    core: &Core,
    deal: &str,
    amount: Decimal,
    since: chrono::DateTime<Utc>,
) -> credit_watch::Ticket {
    let t = credit_watch::register(
        core,
        deal,
        &VenueId("bingx".into()),
        &VenueId("bsc_dex".into()),
        &asset(usdt()),
        amount,
    );
    t.set_account(Some(DexAccountId(WALLET.into())));
    t.set_chain(since);
    t
}

fn ours() -> Decimal {
    "58.69".parse().unwrap()
}

fn before() -> chrono::DateTime<Utc> {
    Utc::now() - Duration::seconds(1)
}

#[tokio::test]
async fn right_amount_credits_without_sender_hash() {
    let (core, s) = start("ok").await;
    configure(&s, true).await;
    let t = ticket(&core, "d1", ours(), before());
    credit_event::on_change(&core, deposit(OUR, usdt(), ours(), WALLET)).await;
    let hit = t.chain_hit().expect("зачислено по блокчейну");
    assert_eq!(hit.tx_hash.as_deref(), Some(OUR));
    assert_eq!(hit.delta, ours());
    assert!(
        transfer_credit::by_event(Some(&t)).is_none(),
        "решение 76 не тронуто"
    );
}

#[tokio::test]
async fn foreign_amount_or_token_not_credited() {
    let (core, s) = start("foreign").await;
    configure(&s, true).await;
    let all = transfers();
    let usdt_other = all
        .iter()
        .find(|(h, tok, a)| h != OUR && *tok == usdt() && *a != ours())
        .unwrap();
    let token_other = all.iter().find(|(_, tok, _)| *tok != usdt()).unwrap();
    let t = ticket(&core, "d1", ours(), before());
    credit_event::on_change(&core, deposit(&usdt_other.0, usdt(), usdt_other.2, WALLET)).await;
    assert!(t.chain_hit().is_none(), "чужое количество");
    credit_event::on_change(
        &core,
        deposit(&token_other.0, token_other.1, ours(), WALLET),
    )
    .await;
    assert!(t.chain_hit().is_none(), "чужой токен");
    credit_event::on_change(&core, deposit(OUR, usdt(), ours(), "mexc")).await;
    assert!(t.chain_hit().is_none(), "другой кошелёк");
}

#[tokio::test]
async fn second_expected_inflow_not_credited() {
    let (core, s) = start("second").await;
    configure(&s, true).await;
    let other = transfers()
        .into_iter()
        .find(|(h, tok, a)| h != OUR && *tok == usdt() && *a != ours())
        .unwrap();
    let t = ticket(&core, "d1", ours(), before());
    let t2 = ticket(&core, "d2", other.2, before());
    credit_event::on_change(&core, deposit(OUR, usdt(), ours(), WALLET)).await;
    assert!(
        t.chain_hit().is_none(),
        "ожидается ещё приход того же токена"
    );
    assert!(t2.chain_hit().is_none());
}

#[tokio::test]
async fn before_withdraw_or_switch_off_not_credited() {
    let (core, s) = start("off").await;
    configure(&s, true).await;
    let t = ticket(&core, "d1", ours(), Utc::now() + Duration::seconds(60));
    credit_event::on_change(&core, deposit(OUR, usdt(), ours(), WALLET)).await;
    assert!(t.chain_hit().is_none(), "приход раньше запроса вывода");
    drop(t);
    configure(&s, false).await;
    let t = ticket(&core, "d2", ours(), before());
    credit_event::on_change(&core, deposit(OUR, usdt(), ours(), WALLET)).await;
    assert!(t.chain_hit().is_none(), "transfer.credit_by_chain = нет");
}
