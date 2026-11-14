//! Файл: tests/core_usdt_return_tax_277.rs
//! Что делает: решение 277 — налог перевода котируемой монеты USDT при возврате с биржи на кошелёк DEX берётся из
//! замера проверки налогов служебной монеты (venues/bsc_dex/check/service_tax.rs → событие CoinCheck ядру), а не
//! «налог перевода не проверен». Живой бот 14.11.2026: 6 возвратов USDT (bingx ×3, mexc ×3) шли «зачисление USDT только
//! по опросу статуса — USDT: налог перевода не проверен», и поступление на кошелёк писалось Warn core.balance_external.
//! Данные — только записанные: справочник сетей BingX (tests/data/bingx_2026-10-24/config_getall.json: USDT BEP20,
//! комиссия вывода 0.15, адрес 0x55d3…7955) и живой ответ узла на пробу переводов USDT
//! (tests/data/service_tax_2026-10-25/responses.json: налоги 0, разбор service_tax::read — как в адаптере).
//! - налог USDT не измерен: ожидания нет (как раньше), причина — «налог перевода не проверен»;
//! - налог измерен: ожидание регистрируется, приход = сумма − комиссия вывода, поступление на кошелёк прикрепляется к
//!   возврату (не внешнее изменение);
//! - проверка USDT — is_quote (экрану «Монеты» и пересборке не передаётся, core/tick.rs), проверка другой монеты — нет.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::venues::{bingx, bsc_dex}, arb::settings, arb::storage. Моков нет: площадок в
//! шлюзе нет — проверяется решение ядра (комиссия вывода — из справочника сетей ядра, credit_expect.rs).

use alloy_primitives::{Address, U256, address};
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use arb::core::Core;
use arb::core::deps::Deps;
use arb::core::types::{
    Asset, BalanceChange, BalanceKind, CheckState, CoinCheck, CoinNetworkInfo, VenueId,
};
use arb::core::{credit_event, credit_expect, transfer_tax};
use arb::gateway::contract::VenueKind;
use arb::gateway::events::MarketEvent;
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::venues::bsc_dex::check::service_tax;
use arb::venues::bsc_dex::check::simulate::CallResult;

/// USDT BSC — адрес USDT у BingX в сети BEP20 (config_getall.json, contractAddress).
const USDT: Address = address!("0x55d398326f99059fF775485246999027B3197955");

fn v(s: &str) -> VenueId {
    VenueId(s.into())
}

fn usdt() -> Asset {
    Asset {
        ticker: "USDT".into(),
        bsc_contract: Some(USDT),
    }
}

fn json_file(path: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).expect("файл записи")).expect("JSON")
}

/// Сеть USDT BEP20 BingX из записи (разбор адаптера BingX).
fn bingx_usdt() -> (Vec<CoinNetworkInfo>, CoinNetworkInfo) {
    let b = json_file("tests/data/bingx_2026-10-24/config_getall.json");
    let list =
        arb::venues::bingx::wallet::parse_networks(&b["data"], "BEP20", &v("bingx")).unwrap();
    let one = list
        .iter()
        .find(|n| n.asset == usdt())
        .expect("USDT BEP20 в записи")
        .clone();
    (list, one)
}

/// Проверка налогов USDT из живого ответа узла — тем же разбором, что адаптер (service_tax::read); сумма пробы — как в
/// tests/bsc_dex_service_tax.rs (5·10^18).
fn usdt_check() -> CoinCheck {
    let rec = json_file("tests/data/service_tax_2026-10-25/responses.json");
    let r: Vec<CallResult> =
        serde_json::from_value(rec["response"]["result"][0]["calls"].clone()).unwrap();
    let amount = U256::from(5u8) * U256::from(10u8).pow(U256::from(18u8));
    let (t, _) = service_tax::read(&r, amount, 18).unwrap();
    CoinCheck {
        venue: v("bsc_dex"),
        asset: usdt(),
        taxes: CheckState::Known(t),
        source: arb::venues::bsc_dex::check::taxes::SOURCE.into(),
        checked_at: Some(Utc::now()),
        wait: None,
    }
}

/// Ядро на базе во временной папке: виды площадок, справочник сетей BingX, адрес USDT; у DEX — события баланса,
/// допуск 0 и закрытие по событию (решение 76).
async fn start(name: &str) -> Core {
    let d = std::env::temp_dir().join(format!("arb-t277-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    let dex = Scope::Venue("bsc_dex".into());
    for (n, val) in [
        ("venue.balance_events", json!(true)),
        ("transfer.credit_amount_tolerance_pct", json!("0")),
        ("transfer.credit_by_event", json!(true)),
    ] {
        settings
            .set(n, dex.clone(), Some(val), "тест")
            .await
            .unwrap();
    }
    let notifier = Notifier {
        http: reqwest::Client::new(),
        settings: settings.clone(),
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        journal: journal.clone(),
    };
    let core = Core::start(Deps {
        gw: Registry::new(),
        settings,
        journal,
        notifier,
        db,
        changes,
        buffer: 64,
        data_dir: d.to_path_buf(),
    })
    .await;
    *core.inner.kinds.write().unwrap() =
        vec![(v("bingx"), VenueKind::Cex), (v("bsc_dex"), VenueKind::Dex)];
    core.inner
        .networks
        .write()
        .unwrap()
        .insert(v("bingx"), std::sync::Arc::new(bingx_usdt().0));
    *core.inner.usdt.write().unwrap() = Ok(USDT);
    core
}

/// Поступление USDT на кошелёк DEX (событие баланса без хеша).
fn deposit(delta: Decimal) -> BalanceChange {
    BalanceChange {
        venue: v("bsc_dex"),
        asset: usdt(),
        delta,
        kind: BalanceKind::Deposit,
        own: false,
        tx_hash: None,
        at: Utc::now(),
        source: "тест".into(),
        account: None,
    }
}

#[tokio::test]
async fn unmeasured_usdt_tax_keeps_poll_only() {
    let core = start("off").await;
    let (_, net) = bingx_usdt();
    let qty = Decimal::new(100, 0);
    let e = credit_expect::arrival(
        &core,
        &v("bingx"),
        &v("bsc_dex"),
        &usdt(),
        &net.network,
        qty,
    )
    .await
    .unwrap_err();
    assert!(e.contains("налог перевода не проверен"), "{e}");
    let t = credit_expect::ticket(
        &core,
        "d1",
        &v("bingx"),
        &v("bsc_dex"),
        &usdt(),
        &net.network,
        qty,
    )
    .await;
    assert!(t.is_none(), "без замера налога — только опрос, как раньше");
}

#[tokio::test]
async fn measured_usdt_tax_registers_and_matches() {
    let core = start("on").await;
    core.inner
        .market
        .write()
        .unwrap()
        .apply(MarketEvent::CoinCheck(usdt_check()));
    let (_, net) = bingx_usdt();
    let qty = Decimal::new(100, 0);
    let fee = net.fee_for(qty).unwrap();
    // Налог USDT измерен живым узлом — 0: приход = сумма − комиссия вывода BingX.
    let got = credit_expect::arrival(
        &core,
        &v("bingx"),
        &v("bsc_dex"),
        &usdt(),
        &net.network,
        qty,
    )
    .await
    .unwrap();
    assert_eq!(got, qty - fee);
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
    .expect("ожидание зачисления зарегистрировано");
    credit_event::on_change(&core, deposit(got)).await;
    assert!(
        t.has_event(),
        "поступление прикреплено к возврату, не внешнее изменение"
    );
}

#[tokio::test]
async fn usdt_check_is_quote_other_coin_not() {
    let core = start("quote").await;
    let c = usdt_check();
    assert!(transfer_tax::is_quote(&core, &c));
    let mut other = c.clone();
    // Другой адрес из той же записи: пул-источник пробы (PancakeSwap V2 WBNB/USDT, tests/bsc_dex_service_tax.rs).
    other.asset.bsc_contract = Some(address!("0x16b9a82891338f9ba80e2d6970fdda79d1eb0dae"));
    assert!(!transfer_tax::is_quote(&core, &other));
}
