//! Файл: tests/core_credit_chain_418.rs
//! Что делает: решение 418 — зачисление по блокчейну (решение 151) не отклоняется из-за других ожиданий того же токена
//! на том же кошельке, если количество события к ним не подходит (credit_match::fits, тот же допуск). Случай из
//! журнала сделки 20261119172323559SINGULARRY (MEXC → DEX, 19.11): на кошельке три ожидания одного токена —
//! 12 493,47 (эта сделка), 554,59 (сделка 17:23:28) и около 3 тыс. (сделка 17:23:52; в журнале — «~3 тыс.», здесь
//! 3000); Transfer 12 493,47 тогда не засчитан — «ожидается ещё 2 поступлений».
//! - Transfer 12 493,47 — засчитан своей сделке сразу; соседние не тронуты; следом Transfer 554,59 — засчитан своей;
//! - Transfer, подходящий к двум ожиданиям (одинаковые количества), — не засчитан, ждём хеш.
//! Количества — из журнала сделки; хеши событий и адрес токена — настоящие из записи квитанций блока 129702236
//! (tests/data/receipts_2026-10-25/responses.json; адрес контракта SINGULARRY в записях нет — не придумываем, берём
//! записанный адрес токена: проверяется сверка по количеству, не сам токен).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::settings, arb::storage, tests/receipts_common. Моков нет: площадок в реестре нет —
//! проверяется решение ядра по событию (как tests/core_credit_chain.rs).

mod receipts_common;

use alloy_primitives::Address;
use chrono::{Duration, Utc};
use rust_decimal::Decimal;
use serde_json::json;
use tokio::sync::broadcast;

use arb::core::Core;
use arb::core::deps::Deps;
use arb::core::types::{Asset, BalanceChange, BalanceKind, DexAccountId, VenueId};
use arb::core::{credit_event, credit_watch};
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use receipts_common::{load, result};

const USDT: &str = "0x55d398326f99059ff775485246999027b3197955";
/// Кошелёк сделки (id счёта DEX из маппинга биржи-отправителя).
const WALLET: &str = "mexc";

/// Ядро на базе во временной папке и хранилище настроек.
async fn start(name: &str) -> (Core, Store) {
    let d = std::env::temp_dir().join(format!("arb-chain418-{name}-{}", std::process::id()));
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
    let v = Scope::Venue("bsc_dex".into());
    let set = [
        ("transfer.credit_amount_tolerance_pct", json!("0")),
        ("transfer.credit_by_event", json!(true)),
        ("transfer.credit_by_chain", json!(true)),
    ];
    for (n, value) in set {
        settings
            .set(n, v.clone(), Some(value), "тест")
            .await
            .unwrap();
    }
    (core, settings)
}

/// Хеши транзакций блока 129702236 из записи (первые два разных).
fn hashes() -> (String, String) {
    let receipts = result(&load("responses.json"), "receipts 129702236");
    let all: Vec<String> = receipts
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["transactionHash"].as_str().unwrap().to_string())
        .collect();
    (all[0].clone(), all[1].clone())
}

fn asset() -> Asset {
    Asset {
        ticker: "USDT".into(),
        bsc_contract: Some(USDT.parse::<Address>().unwrap()),
    }
}

fn d(s: &str) -> Decimal {
    s.parse().unwrap()
}

/// Поступление на кошелёк сделки. Принимает: хеш, сумму.
fn deposit(hash: &str, delta: Decimal) -> BalanceChange {
    BalanceChange {
        venue: VenueId("bsc_dex".into()),
        asset: asset(),
        delta,
        kind: BalanceKind::Deposit,
        own: false,
        tx_hash: Some(hash.to_string()),
        at: Utc::now(),
        source: "Transfer в блоке 129702236".into(),
        account: Some(DexAccountId(WALLET.into())),
    }
}

/// Ожидание перевода сделки mexc → DEX, «вооружённое» (решение 151) секунду назад.
fn ticket(core: &Core, deal: &str, amount: Decimal) -> credit_watch::Ticket {
    let t = credit_watch::register(
        core,
        deal,
        &VenueId("mexc".into()),
        &VenueId("bsc_dex".into()),
        &asset(),
        amount,
    );
    t.set_account(Some(DexAccountId(WALLET.into())));
    t.set_chain(Utc::now() - Duration::seconds(1));
    t
}

#[tokio::test]
async fn singularry_amount_ours_credited_at_once() {
    let (core, _s) = start("ours").await;
    let (h1, h2) = hashes();
    let big = ticket(&core, "SINGULARRY-172323", d("12493.47"));
    let small = ticket(&core, "SINGULARRY-172328", d("554.59"));
    let mid = ticket(&core, "SINGULARRY-172352", d("3000"));
    credit_event::on_change(&core, deposit(&h1, d("12493.47"))).await;
    let hit = big
        .chain_hit()
        .expect("количество однозначно наше — засчитано по блокчейну");
    assert_eq!(hit.delta, d("12493.47"));
    assert_eq!(hit.tx_hash.as_deref(), Some(h1.as_str()));
    assert!(
        small.chain_hit().is_none() && !small.has_event(),
        "соседняя не тронута"
    );
    assert!(
        mid.chain_hit().is_none() && !mid.has_event(),
        "соседняя не тронута"
    );
    // Соседняя сделка зачисляется своим Transfer.
    credit_event::on_change(&core, deposit(&h2, d("554.59"))).await;
    let hit2 = small
        .chain_hit()
        .expect("соседняя засчитана своим Transfer");
    assert_eq!(hit2.delta, d("554.59"));
    assert_eq!(
        big.chain_hit().unwrap().delta,
        d("12493.47"),
        "первая не изменилась"
    );
    assert!(mid.chain_hit().is_none());
}

#[tokio::test]
async fn amount_fits_two_waits_for_hash() {
    let (core, _s) = start("two").await;
    let (h1, _) = hashes();
    let big = ticket(&core, "d1", d("12493.47"));
    let a = ticket(&core, "d2", d("554.59"));
    let b = ticket(&core, "d3", d("554.59"));
    credit_event::on_change(&core, deposit(&h1, d("554.59"))).await;
    assert!(a.chain_hit().is_none(), "подходит к двум — ждём хеш");
    assert!(b.chain_hit().is_none(), "подходит к двум — ждём хеш");
    assert!(big.chain_hit().is_none());
}
