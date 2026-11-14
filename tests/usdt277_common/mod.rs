//! Файл: tests/usdt277_common/mod.rs
//! Что делает: общее для тестов решения 277 (налог перевода USDT при возврате биржа → кошелёк DEX): ядро на SQLite во
//! временной папке, справочник сетей BingX и проверка налогов USDT — только из записей:
//! - tests/data/bingx_2026-10-24/config_getall.json — USDT BEP20 BingX (комиссия вывода 0.15, адрес 0x55d3…7955);
//! - tests/data/service_tax_2026-10-25/responses.json — живой ответ узла на пробу переводов USDT (налоги 0), разбор
//!   service_tax::read, как в адаптере.
//!
//! Кто вызывает: tests/core_usdt_return_tax_277.rs, core_usdt_return_tax_277_hash.rs, core_usdt_return_tax_277_cex.rs.
//! От чего зависит: arb::core, arb::venues::{bingx, bsc_dex}, arb::settings, arb::storage.
#![allow(dead_code)]

use alloy_primitives::{Address, U256, address};
use chrono::Utc;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use arb::core::Core;
use arb::core::deps::Deps;
use arb::core::types::{Asset, CheckState, CoinCheck, CoinNetworkInfo, TokenTaxes, VenueId};
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
pub const USDT: Address = address!("0x55d398326f99059fF775485246999027B3197955");

pub fn v(s: &str) -> VenueId {
    VenueId(s.into())
}

pub fn usdt() -> Asset {
    Asset {
        ticker: "USDT".into(),
        bsc_contract: Some(USDT),
    }
}

fn json_file(path: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).expect("файл записи")).expect("JSON")
}

/// Сеть USDT BEP20 BingX из записи (разбор адаптера BingX): (весь справочник, запись USDT).
pub fn bingx_usdt() -> (Vec<CoinNetworkInfo>, CoinNetworkInfo) {
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

/// Налоги USDT из живого ответа узла — тем же разбором, что адаптер (service_tax::read); сумма пробы — как в
/// tests/bsc_dex_service_tax.rs (5·10^18).
pub fn usdt_taxes() -> TokenTaxes {
    let rec = json_file("tests/data/service_tax_2026-10-25/responses.json");
    let r: Vec<CallResult> =
        serde_json::from_value(rec["response"]["result"][0]["calls"].clone()).unwrap();
    let amount = U256::from(5u8) * U256::from(10u8).pow(U256::from(18u8));
    service_tax::read(&r, amount, 18).unwrap().0
}

/// Проверка налогов USDT площадки bsc_dex с данными налогами.
pub fn usdt_check_with(t: TokenTaxes) -> CoinCheck {
    CoinCheck {
        venue: v("bsc_dex"),
        asset: usdt(),
        taxes: CheckState::Known(t),
        source: arb::venues::bsc_dex::check::taxes::SOURCE.into(),
        checked_at: Some(Utc::now()),
        wait: None,
    }
}

/// Проверка налогов USDT из записи (налоги 0).
pub fn usdt_check() -> CoinCheck {
    usdt_check_with(usdt_taxes())
}

/// Проверку — в рынок ядра (как tick.rs по событию CoinCheck).
pub fn apply(core: &Core, c: CoinCheck) {
    core.inner
        .market
        .write()
        .unwrap()
        .apply(MarketEvent::CoinCheck(c));
}

/// Настройка площадки. Принимает: ядро, площадку (None — вся система), имя, значение.
pub async fn set(core: &Core, venue: Option<&str>, name: &str, val: Value) {
    let scope = venue.map_or(Scope::Global, |x| Scope::Venue(x.into()));
    core.inner
        .deps
        .settings
        .set(name, scope, Some(val), "тест")
        .await
        .unwrap();
}

/// Ядро: виды площадок (bingx и mexc — биржи, bsc_dex — DEX), справочник сетей BingX, адрес USDT; у DEX — события
/// баланса, допуск 0 и закрытие по событию (решение 76).
pub async fn start(name: &str) -> Core {
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
    for (n, val) in [
        ("venue.balance_events", json!(true)),
        ("transfer.credit_amount_tolerance_pct", json!("0")),
        ("transfer.credit_by_event", json!(true)),
    ] {
        set(&core, Some("bsc_dex"), n, val).await;
    }
    *core.inner.kinds.write().unwrap() = vec![
        (v("bingx"), VenueKind::Cex),
        (v("mexc"), VenueKind::Cex),
        (v("bsc_dex"), VenueKind::Dex),
    ];
    core.inner
        .networks
        .write()
        .unwrap()
        .insert(v("bingx"), std::sync::Arc::new(bingx_usdt().0));
    *core.inner.usdt.write().unwrap() = Ok(USDT);
    core
}
