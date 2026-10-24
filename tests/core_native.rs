//! Файл: tests/core_native.rs
//! Что делает: нативная монета в справочнике ядра (решение 62) на настоящей SQLite во временной папке:
//! пара настроек dex.native_ticker + dex.native_wrapped площадки DEX → запись биржи «BNB, сеть BSC, адреса нет» получает
//! адрес WBNB; остальные монеты и другие сети не трогаются; без настроек, без сети BSC биржи или если адрес обёртки
//! уже занят другой монетой биржи — сопоставления нет.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::native, arb::settings, arb::storage. Моков нет.
//! Записи биржи — в формате CoinNetworkInfo, который адаптер BingX строит из config/getall (venues/bingx/wallet.rs):
//! записанного ответа config/getall в tests/data пока нет (запрошен у Тиграна, сырой журнал REST BingX — PR #41);
//! по словам Тиграна, BNB там — сеть BEP20 с пустым contractAddress.

use alloy_primitives::{Address, address};
use chrono::Utc;
use serde_json::json;
use tokio::sync::broadcast;

use arb::core::native::{self, Native};
use arb::core::types::{Asset, CoinNetworkInfo, VenueId};
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;

const WBNB: Address = address!("0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c");
const CAKE: Address = address!("0x0E09FaBB73Bd3Ade0a17ECC321fD13a19e81cE82");

/// Новая база во временной папке.
async fn fresh(name: &str) -> Store {
    let dir = std::env::temp_dir().join(format!("arb-native-core-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("t.sqlite")).unwrap();
    let (tx, _) = broadcast::channel(16);
    let (jtx, _) = broadcast::channel(16);
    Store::load(db.clone(), tx, Journal::new(db, jtx))
        .await
        .unwrap()
}

/// Запись сети биржи.
fn net(ticker: &str, network: &str, contract: Option<Address>) -> CoinNetworkInfo {
    CoinNetworkInfo {
        venue: VenueId("bingx".into()),
        asset: Asset {
            ticker: ticker.into(),
            bsc_contract: contract,
        },
        network: network.into(),
        deposit_enabled: true,
        withdraw_enabled: true,
        withdraw_fee: None,
        withdraw_min: None,
        withdraw_max: None,
        deposit_min: None,
        deposit_confirmations: None,
        updated_at: Utc::now(),
    }
}

#[tokio::test]
async fn settings_pair_required() {
    let s = fresh("read").await;
    let dex = [VenueId("bsc_dex".into())];
    assert_eq!(native::read(&s, &dex), Ok(None));
    let sc = Scope::Venue("bsc_dex".into());
    s.set("dex.native_ticker", sc.clone(), Some(json!("BNB")), "тест")
        .await
        .unwrap();
    assert_eq!(native::read(&s, &dex), Ok(None), "без адреса обёртки — нет");
    s.set(
        "dex.native_wrapped",
        sc,
        Some(json!(WBNB.to_string())),
        "тест",
    )
    .await
    .unwrap();
    assert_eq!(
        native::read(&s, &dex),
        Ok(Some(Native {
            ticker: "BNB".into(),
            wrapped: WBNB
        }))
    );
}

#[test]
fn only_native_row_on_bsc_network() {
    let n = Native {
        ticker: "BNB".into(),
        wrapped: WBNB,
    };
    let list = vec![
        net("BNB", "BEP20", None),
        net("BNB", "OPBNB", None),
        net("CAKE", "BEP20", Some(CAKE)),
        net("FOO", "BEP20", None),
    ];
    let out = native::apply(list.clone(), Some("BEP20"), &n);
    assert_eq!(out[0].asset.bsc_contract, Some(WBNB));
    assert_eq!(out[1].asset.bsc_contract, None, "другая сеть — без адреса");
    assert_eq!(out[2].asset.bsc_contract, Some(CAKE));
    assert_eq!(
        out[3].asset.bsc_contract, None,
        "не нативная — строго по адресу"
    );
    // Сеть BSC биржи не задана — сопоставления нет.
    assert_eq!(
        native::apply(list.clone(), None, &n)[0].asset.bsc_contract,
        None
    );
    // Адрес обёртки уже у другой монеты биржи — две монеты на одном адресе не склеиваются.
    let mut taken = list;
    taken.push(net("WBNB", "BEP20", Some(WBNB)));
    assert_eq!(
        native::apply(taken, Some("BEP20"), &n)[0]
            .asset
            .bsc_contract,
        None
    );
}
