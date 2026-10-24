//! Файл: tests/core_native.rs
//! Что делает: нативная монета в справочнике ядра (решение 62) на настоящей SQLite во временной папке:
//! пара настроек dex.native_ticker + dex.native_wrapped площадки DEX → запись биржи «BNB, сеть BSC, адреса нет» получает
//! адрес WBNB; без настроек, без сети BSC биржи или если она названа иначе — сопоставления нет; другие сети BNB
//! (закрытые XTERIOB и Opbnb) не трогаются.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::native, arb::settings, arb::storage. Моков нет.
//! Записи биржи — настоящие ответы BingX config/getall для BNB (tests/data/native_2026-10-25/bingx_bnb_getall_2026-10-24.json,
//! сырой журнал бота 2026-10-24), разобранные адаптером BingX (venues/bingx/wallet.rs, parse_networks): BEP20 с пустым
//! contractAddress, ввод и вывод открыты; XTERIOB и Opbnb — закрыты.

use alloy_primitives::{Address, address};
use serde_json::{Value, json};
use tokio::sync::broadcast;

use arb::core::native::{self, Native};
use arb::core::types::{CoinNetworkInfo, VenueId};
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::venues::bingx::wallet::parse_networks;

const WBNB: Address = address!("0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c");
const GETALL: &str = "tests/data/native_2026-10-25/bingx_bnb_getall_2026-10-24.json";

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

/// Записи BNB из настоящих ответов BingX config/getall (первая и последняя за 2026-10-24) — разбором адаптера BingX.
fn recorded() -> Vec<Vec<CoinNetworkInfo>> {
    let text = std::fs::read_to_string(GETALL).unwrap();
    let v: Value = serde_json::from_str(&text).unwrap();
    v["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| parse_networks(&r["BNB"], "BEP20", &VenueId("bingx".into())).unwrap())
        .collect()
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
fn recorded_bnb_maps_only_bep20() {
    let n = Native {
        ticker: "BNB".into(),
        wrapped: WBNB,
    };
    let records = recorded();
    assert_eq!(records.len(), 2);
    for list in records {
        // Как пришло от BingX: три сети BNB, ни у одной нет адреса контракта (contractAddress = "").
        let nets: Vec<&str> = list.iter().map(|i| i.network.as_str()).collect();
        assert_eq!(nets, ["BEP20", "XTERIOB", "Opbnb"]);
        assert!(list.iter().all(|i| i.asset.bsc_contract.is_none()));
        let out = native::apply(list.clone(), Some("BEP20"), &n);
        let bep20 = &out[0];
        assert_eq!(bep20.asset.bsc_contract, Some(WBNB));
        assert!(bep20.deposit_enabled && bep20.withdraw_enabled);
        // Другие сети BNB (закрыты у BingX) сопоставление не трогает.
        for other in &out[1..] {
            assert_eq!(other.asset.bsc_contract, None, "{}", other.network);
            assert!(!other.deposit_enabled && !other.withdraw_enabled);
        }
        // Сеть BSC биржи не задана или названа иначе — ни одна сеть не сопоставляется.
        for bsc in [None, Some("BSC")] {
            let out = native::apply(list.clone(), bsc, &n);
            assert!(out.iter().all(|i| i.asset.bsc_contract.is_none()));
        }
        // Другой тикер нативной монеты — BNB не трогается.
        let other = Native {
            ticker: "ETH".into(),
            wrapped: WBNB,
        };
        assert!(
            native::apply(list, Some("BEP20"), &other)
                .iter()
                .all(|i| i.asset.bsc_contract.is_none())
        );
    }
}

/// Газ в BNB у сделки с BNB — затрата (spent), а не уменьшение количества; комиссия биржи в BNB — в количестве
/// (решение 62, core/convert.rs::in_coin — так и в плане core/costs.rs, и в факте core/fills.rs).
#[test]
fn gas_never_in_coin_qty() {
    use arb::core::convert::in_coin;
    use arb::core::types::{Cost, CostKind};
    for k in [
        CostKind::Gas,
        CostKind::ApproveGas,
        CostKind::TransferGas,
        CostKind::WrapGas,
        CostKind::ChannelTip,
    ] {
        assert!(
            !in_coin(&Cost::new(k, 1.into(), "BNB", false), "BNB"),
            "{k:?}"
        );
    }
    assert!(in_coin(
        &Cost::new(CostKind::TradeFee, 1.into(), "BNB", false),
        "BNB"
    ));
    assert!(!in_coin(
        &Cost::new(CostKind::TradeFee, 1.into(), "USDT", false),
        "BNB"
    ));
}
