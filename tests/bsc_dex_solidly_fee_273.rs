//! Файл: tests/bsc_dex_solidly_fee_273.rs
//! Что делает: решение 273 — комиссия Solidly по источнику из настроек и ограничения обмена пары, без сети и без моков,
//! на ЖИВОЙ записи узла BSC (tests/data/solidly_fee_2026-11-14, один блок 133593628, README там же):
//! (1) математика бота (правило комиссии math/solidly_fee.rs + формулы пары с нулевой комиссией — ровно путь
//!   quote/pool_quote.rs) совпадает с getAmountOut пары ДО WEI на 9 пулах трёх DEX (usdfi — делитель, в том числе
//!   fee 333 и 2500, где правила расходятся; veplus, magicfox — числитель над 10000), volatile и stable, обе стороны;
//! (2) правило «числитель над 10000» на парах usdfi НЕ совпадает (без решения 273 пулы считались бы неверно);
//! (3) «1 в 1» с прежним путём: при правиле «числитель над 10000» новый путь даёт те же числа, что прежние функции
//!   с комиссией внутри (solidly_v1_math), на всех записанных образцах veplus/magicfox;
//! (4) проценты комиссии, доля для обратной формулы, проверка значения по правилу, ограничения (пауза, maxGasPrice
//!   5 gwei с цепи), селекторы методов из настроек;
//! (5) реальный Sync-лог каждой DEX разбирается ISolidlyPool::Sync и равен getReserves пары;
//! (6) обязательные настройки DEX по источнику (families::required): не задан источник — только фабрика, как раньше.
//! Кто вызывает: cargo test --test bsc_dex_solidly_fee_273.
//! От чего зависит: arb::venues::bsc_dex (math::{solidly_fee, solidly_v1_math}, pools::{solidly_fee_read,
//! abi_solidly, families}), arb::core::types, arb::settings (настоящая SQLite во временной папке).

use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use arb::core::types::{FeeRule, SolidlyFee, SolidlyGate};
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::math::{solidly_fee, solidly_v1_math};
use arb::venues::bsc_dex::pools::abi_solidly::ISolidlyPool;
use arb::venues::bsc_dex::pools::families;
use arb::venues::bsc_dex::pools::solidly_fee_read::{checked, selector};

const DATA: &str = "tests/data/solidly_fee_2026-11-14";
const NUM: FeeRule = FeeRule::Numerator { denominator: 10_000 };

fn load(name: &str) -> Value {
    let raw = std::fs::read_to_string(format!("{DATA}/{name}")).unwrap();
    serde_json::from_str(&raw).unwrap()
}

fn u(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}

/// Комиссия пула записи по правилу его DEX: usdfi — fee() делителем, veplus — swapFee() числителем над 10000,
/// magicfox — stableFee()/volatileFee() фабрики числителем над 10000.
fn fee_of(dex: &str, d: &Value, p: &Value) -> SolidlyFee {
    let stable = p["metadata"]["st"].as_bool().unwrap();
    let (raw, rule) = match dex {
        "usdfi" => (u(&p["fee"]), FeeRule::Divisor),
        "veplus" => (u(&p["fee"]), NUM),
        _ => {
            let m = if stable { "stableFee()" } else { "volatileFee()" };
            (u(&d["factoryCalls"][m]), NUM)
        }
    };
    SolidlyFee { value: raw.to::<u32>(), rule }
}

/// Выход пары путём quote/pool_quote.rs: вход после комиссии по правилу, формула пары с нулевой комиссией.
fn ours(fee: &SolidlyFee, p: &Value, amount: U256, token_in: &str) -> Option<U256> {
    let m = &p["metadata"];
    let zfo = token_in == m["t0"].as_str().unwrap();
    let (r_in, r_out, d_in, d_out) = if zfo {
        (u(&m["r0"]), u(&m["r1"]), u(&m["dec0"]), u(&m["dec1"]))
    } else {
        (u(&m["r1"]), u(&m["r0"]), u(&m["dec1"]), u(&m["dec0"]))
    };
    let net = solidly_fee::net_in(amount, fee)?;
    if m["st"].as_bool().unwrap() {
        solidly_v1_math::stable_amount_out(net, r_in, r_out, d_in, d_out, U256::ZERO)
    } else {
        solidly_v1_math::volatile_amount_out(net, r_in, r_out, U256::ZERO)
    }
}

/// Все образцы записи: (dex, данные DEX, пул, вход, токен входа, getAmountOut цепи).
fn samples(s: &Value) -> Vec<(String, Value, Value, U256, String, U256)> {
    let mut out = Vec::new();
    for (dex, d) in s["dex"].as_object().unwrap() {
        for p in d["pools"].as_array().unwrap() {
            for q in p["getAmountOut"].as_array().unwrap() {
                let t = q[1].as_str().unwrap().to_string();
                out.push((dex.clone(), d.clone(), p.clone(), u(&q[0]), t, u(&q[2])));
            }
        }
    }
    out
}

/// (1) До wei с getAmountOut цепи на всех 36 образцах; metadata и getReserves одного блока совпадают.
#[test]
fn bot_math_equals_get_amount_out_to_wei() {
    let s = load("state.json");
    assert_eq!(s["block"], 133593628);
    let all = samples(&s);
    assert_eq!(all.len(), 36, "9 пулов × 2 стороны × 2 суммы");
    let (mut st, mut vol) = (0, 0);
    for (dex, d, p, a, t, want) in &all {
        let m = &p["metadata"];
        assert_eq!(p["getReserves"][0], m["r0"]);
        assert_eq!(p["getReserves"][1], m["r1"]);
        let fee = fee_of(dex, d, p);
        let got = ours(&fee, p, *a, t).unwrap();
        assert_eq!(got, *want, "{dex} {} вход {a} {t}", p["pool"]);
        if m["st"].as_bool().unwrap() { st += 1 } else { vol += 1 }
    }
    assert!(st >= 12 && vol >= 12, "и stable, и volatile: {st}/{vol}");
}

/// (2) У usdfi правило «числитель над 10000» дало бы другой выход (fee 333 и 2500 — заведомо).
#[test]
fn usdfi_numerator_rule_does_not_match_chain() {
    let s = load("state.json");
    for (dex, d, p, a, t, want) in samples(&s).iter().filter(|x| x.0 == "usdfi") {
        let f = fee_of(dex, d, p);
        let wrong = SolidlyFee { value: f.value, rule: NUM };
        assert_ne!(ours(&wrong, p, *a, t), Some(*want), "{} fee {}", p["pool"], f.value);
    }
    let fees: Vec<u32> = s["dex"]["usdfi"]["pools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| u(&p["fee"]).to::<u32>())
        .collect();
    assert_eq!(fees, vec![2500, 333, 10000]);
}

/// (3) Правило «числитель над 10000» — те же числа, что прежние функции с комиссией внутри (путь до решения 273).
#[test]
fn numerator_10000_is_byte_identical_to_previous_path() {
    let s = load("state.json");
    let mut n = 0;
    for (dex, d, p, a, t, want) in samples(&s).iter().filter(|x| x.0 != "usdfi") {
        let f = fee_of(dex, d, p);
        let m = &p["metadata"];
        let zfo = t == m["t0"].as_str().unwrap();
        let (r_in, r_out, d_in, d_out) = if zfo {
            (u(&m["r0"]), u(&m["r1"]), u(&m["dec0"]), u(&m["dec1"]))
        } else {
            (u(&m["r1"]), u(&m["r0"]), u(&m["dec1"]), u(&m["dec0"]))
        };
        let old = if m["st"].as_bool().unwrap() {
            solidly_v1_math::stable_amount_out(*a, r_in, r_out, d_in, d_out, U256::from(f.value))
        } else {
            solidly_v1_math::volatile_amount_out(*a, r_in, r_out, U256::from(f.value))
        };
        assert_eq!(ours(&f, p, *a, t), old);
        assert_eq!(old, Some(*want));
        n += 1;
    }
    assert_eq!(n, 24);
}

/// (4) Проценты, доля для обратной формулы, проверка значения по правилу, ограничения с цепи, селекторы.
#[tokio::test]
async fn fee_helpers_and_gate() {
    let num = |v| SolidlyFee { value: v, rule: NUM };
    let div = |v| SolidlyFee { value: v, rule: FeeRule::Divisor };
    // Как до решения 273: fee / 100.
    assert_eq!(solidly_fee::pct(&num(20)), Some(Decimal::from(20) / Decimal::from(100)));
    assert_eq!(solidly_fee::pct(&div(2500)), Some(Decimal::new(4, 2)));
    assert_eq!(solidly_fee::pct(&div(100)), Some(Decimal::ONE));
    assert_eq!(solidly_fee::pct(&div(0)), None);
    assert_eq!(solidly_fee::keep_ratio(&num(30)), Some((U256::from(9970), U256::from(10000))));
    assert_eq!(solidly_fee::keep_ratio(&div(333)), Some((U256::from(332), U256::from(333))));
    assert_eq!(solidly_fee::net_in(U256::from(1000), &div(0)), None);

    let c = ctx("chk").await;
    assert_eq!(checked(&c, U256::from(9999), NUM).unwrap().value, 9999);
    assert!(checked(&c, U256::from(10_000), NUM).is_err());
    assert!(checked(&c, U256::from(1), FeeRule::Divisor).is_err());
    assert_eq!(checked(&c, U256::from(2), FeeRule::Divisor).unwrap().value, 2);

    // usdfi: isPaused() = 0, maxGasPrice() = 5 gwei на блоке записи.
    let s = load("state.json");
    let fc = &s["dex"]["usdfi"]["factoryCalls"];
    assert_eq!(fc["isPaused()"], "0");
    let max = u(&fc["maxGasPrice()"]).to::<u64>();
    assert_eq!(max, 5_000_000_000);
    let g = SolidlyGate { paused: false, max_gas_price: Some(max) };
    assert_eq!(solidly_fee::blocked(&g, Some(U256::from(max))), None);
    let over = solidly_fee::blocked(&g, Some(U256::from(max + 1))).unwrap();
    assert!(over.contains("maxGasPrice"), "{over}");
    let paused = SolidlyGate { paused: true, max_gas_price: None };
    assert!(solidly_fee::blocked(&paused, None).unwrap().contains("isPaused"));
    assert_eq!(solidly_fee::blocked(&SolidlyGate::default(), Some(U256::MAX)), None);
    // Прежние getFee у всех трёх фабрик откатываются (поэтому и нужен источник из настроек).
    for dex in ["usdfi", "veplus", "magicfox"] {
        let fc = &s["dex"][dex]["factoryCalls"];
        assert_eq!(fc["getFee(bool)[true]"], "revert", "{dex}");
        assert_eq!(fc["getFee(address,bool)"], "revert", "{dex}");
    }
    // Селекторы сигнатур из настроек — как в задаче (сверено с цепью).
    let hex = |sig| alloy_primitives::hex::encode(selector(sig));
    assert_eq!(hex("fee()"), "ddca3f43");
    assert_eq!(hex("swapFee()"), "54cf2aeb");
    assert_eq!(hex("isPaused()"), "b187bd26");
    assert_eq!(hex("maxGasPrice()"), "3de39c11");
}

/// (5) Реальный Sync каждой DEX: разбор по ABI = getReserves пары (у usdfi/veplus — на блоке лога, у всех — на блоке
/// записи: последний Sync пары до головы 133594091).
#[test]
fn real_sync_logs_decode_to_reserves() {
    let l = load("sync.json");
    let s = load("state.json");
    for dex in ["usdfi", "veplus", "magicfox"] {
        let x = &l[dex];
        assert_eq!(x["syncsInBlock"], 1);
        let topic: alloy_primitives::B256 = x["topics"][0].as_str().unwrap().parse().unwrap();
        assert_eq!(topic, ISolidlyPool::Sync::SIGNATURE_HASH);
        let data: alloy_primitives::Bytes = x["data"].as_str().unwrap().parse().unwrap();
        let ev = ISolidlyPool::Sync::abi_decode_data(&data).unwrap();
        let addr: Address = x["address"].as_str().unwrap().parse().unwrap();
        let p = s["dex"][dex]["pools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["pool"].as_str().unwrap().parse::<Address>().unwrap() == addr)
            .unwrap();
        assert_eq!((ev.0, ev.1), (u(&p["getReserves"][0]), u(&p["getReserves"][1])), "{dex}");
        if !x["getReservesAtBlock"].is_null() {
            assert_eq!(ev.0, u(&x["getReservesAtBlock"][0]));
        }
    }
}

/// (6) Обязательные настройки по источнику: не задан — только фабрика (как до решения 273).
#[tokio::test]
async fn required_settings_by_source() {
    let c = ctx("req").await;
    let req = |id: &'static str| families::required(&c, families::make(id, "solidly_v1").unwrap());
    assert_eq!(req("thena-v1"), vec!["dex.factory"]);

    set(&c, "usdfi", &[("dex.solidly_fee_source", json!("pool"))]).await;
    assert_eq!(req("usdfi"), vec!["dex.factory", "dex.solidly_fee_rule", "dex.solidly_fee_method"]);
    set(&c, "usdfi", &[("dex.solidly_fee_rule", json!("divisor"))]).await;
    assert_eq!(req("usdfi"), vec!["dex.factory", "dex.solidly_fee_rule", "dex.solidly_fee_method"]);

    set(&c, "magicfox", &[("dex.solidly_fee_source", json!("factory_by_type")), ("dex.solidly_fee_rule", json!("numerator"))]).await;
    assert_eq!(
        req("magicfox"),
        vec![
            "dex.factory",
            "dex.solidly_fee_rule",
            "dex.solidly_fee_method_stable",
            "dex.solidly_fee_method_volatile",
            "dex.solidly_fee_denominator"
        ]
    );
}

/// Окружение на настоящей SQLite без настроек.
async fn ctx(tag: &str) -> Ctx {
    let d = std::env::temp_dir().join(format!("arb-sol273-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db, changes, journal.clone()).await.unwrap();
    Ctx {
        raw: RawLog::start(d.join("raw"), settings.clone()),
        pace: Default::default(),
        failover: Default::default(),
        account: None,
        index: Default::default(),
        settings,
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        http: reqwest::Client::new(),
        journal,
    }
}

/// Задаёт настройки в области DEX id.
async fn set(c: &Ctx, id: &str, list: &[(&str, Value)]) {
    for (n, v) in list {
        c.settings
            .set(n, Scope::Dex(id.into()), Some(v.clone()), "тест")
            .await
            .unwrap();
    }
}
