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

/// Газ возврата USDT (решение 34) остаётся газом: у сделки с BNB он не в количестве, а предпроверка резерва его видит.
/// Газ — настоящий: перевод 5 USDT в симуляции узла (tests/data/native_2026-10-25, 51591) × цена газа настоящего вывода
/// BNB с BingX (0x5f5e100 wei).
#[test]
fn usdt_return_gas_stays_gas() {
    use arb::core::convert::{in_coin, usdt_return};
    use arb::core::costs::CostLine;
    use arb::core::precheck::gas_need;
    use arb::core::types::{Cost, CostKind};
    use rust_decimal::Decimal;
    let rec: Value = serde_json::from_str(
        &std::fs::read_to_string(
            "tests/data/native_2026-10-25/arb_native_responses_2026-10-25.json",
        )
        .unwrap(),
    )
    .unwrap();
    let sim = rec["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "native wrap/unwrap simulate")
        .unwrap();
    let hex =
        |v: &Value| u64::from_str_radix(v.as_str().unwrap().trim_start_matches("0x"), 16).unwrap();
    let gas = hex(&sim["response"]["result"][0]["calls"][1]["gasUsed"]);
    assert_eq!(gas, 51_591);
    let w: Value = serde_json::from_str(
        &std::fs::read_to_string(
            "tests/data/native_2026-10-25/arb_bingx_bnb_withdraw_2026-10-25.json",
        )
        .unwrap(),
    )
    .unwrap();
    let price = hex(&w["results"][0]["response"]["result"]["gasPrice"]);
    let bnb = Decimal::from(gas * price) / Decimal::from(10u64.pow(18));
    let ret = usdt_return(Cost::new(CostKind::TransferGas, bnb, "BNB", false));
    assert_eq!(ret.kind, CostKind::TransferGas, "газ не переименовывается");
    assert!(
        !in_coin(&ret, "BNB"),
        "газ возврата у сделки с BNB — не в количестве"
    );
    let tax = usdt_return(Cost::new(CostKind::TransferTax, 1.into(), "USDT", false));
    assert_eq!(tax.kind, CostKind::UsdtReturn);
    let line = |c: Cost, stage: &'static str| CostLine {
        kind: c.kind,
        stage,
        amount: c.amount,
        asset: c.asset,
        usdt: None,
        in_price: false,
        in_qty: false,
        step: None,
    };
    let lines = [
        line(ret, "возврат USDT"),
        line(tax, "возврат USDT"),
        line(Cost::new(CostKind::Gas, bnb, "BNB", false), "B"),
        line(Cost::new(CostKind::Gas, bnb, "BNB", false), "A"),
    ];
    assert_eq!(gas_need(&lines, &["B", "возврат USDT"]), bnb + bnb);
    assert_eq!(gas_need(&lines, &["A", "перевод"]), bnb);
}
