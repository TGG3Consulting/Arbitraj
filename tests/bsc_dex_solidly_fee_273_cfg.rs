//! Файл: tests/bsc_dex_solidly_fee_273_cfg.rs
//! Что делает: решение 273 — настройки и ограничения комиссии Solidly, без сети и без моков, на ЖИВОЙ записи узла BSC
//! (tests/data/solidly_fee_2026-11-14, README там же); вынесено из tests/bsc_dex_solidly_fee_273.rs (предел 200 строк):
//! (4) проценты комиссии, доля для обратной формулы, проверка значения по правилу, ограничения (пауза, maxGasPrice
//!   5 gwei с цепи), селекторы методов из настроек; точный текст «процент не считается» для обоих правил;
//! (5) реальный Sync-лог каждой DEX разбирается ISolidlyPool::Sync и равен getReserves пары;
//! (6) обязательные настройки DEX по источнику (families::required): не задан источник — только фабрика, как раньше;
//! (7) ревью 273: запись пула Solidly до решения 273 (`"fee": число`) читается как «числитель над fee_denominator()»,
//!   новая (`{value, rule}`) — как есть, запись — новый вид (состояние пула — из записи узла);
//! (8) ревью 273: maxGasPrice больше u64 — «предела нет», 5 gwei записи — как есть.
//! Кто вызывает: cargo test --test bsc_dex_solidly_fee_273_cfg.
//! От чего зависит: tests/solidly273_common, arb::venues::bsc_dex (math::{solidly_fee, solidly_math}, pools::{solidly_fee_read,
//! abi_solidly, families}), arb::core::types, arb::settings (настоящая SQLite во временной папке).

mod solidly273_common;

use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;
use serde_json::json;

use arb::core::types::{FeeRule, PoolState, SolidlyFee, SolidlyGate};
use arb::venues::bsc_dex::math::{solidly_fee, solidly_math};
use arb::venues::bsc_dex::pools::abi_solidly::ISolidlyPool;
use arb::venues::bsc_dex::pools::families;
use arb::venues::bsc_dex::pools::solidly_fee_read::{checked, max_gas, selector};
use solidly273_common::{NUM, ctx, fee_of, load, set, u};

/// (4) Проценты, доля для обратной формулы, проверка значения по правилу, ограничения с цепи, селекторы.
#[tokio::test]
async fn fee_helpers_and_gate() {
    let num = |v| SolidlyFee {
        value: v,
        rule: NUM,
    };
    let div = |v| SolidlyFee {
        value: v,
        rule: FeeRule::Divisor,
    };
    // Как до решения 273: fee / 100.
    assert_eq!(
        solidly_fee::pct(&num(20)),
        Some(Decimal::from(20) / Decimal::from(100))
    );
    assert_eq!(solidly_fee::pct(&div(2500)), Some(Decimal::new(4, 2)));
    assert_eq!(solidly_fee::pct(&div(100)), Some(Decimal::ONE));
    assert_eq!(solidly_fee::pct(&div(0)), None);
    assert_eq!(
        solidly_fee::keep_ratio(&num(30)),
        Some((U256::from(9970), U256::from(10000)))
    );
    assert_eq!(
        solidly_fee::keep_ratio(&div(333)),
        Some((U256::from(332), U256::from(333)))
    );
    assert_eq!(solidly_fee::net_in(U256::from(1000), &div(0)), None);
    // Ревью 273: точная причина «процент не считается» — своя у каждого правила.
    let zero_den = SolidlyFee {
        value: 5,
        rule: FeeRule::Numerator { denominator: 0 },
    };
    assert_eq!(solidly_fee::pct(&zero_den), None);
    assert!(
        solidly_fee::no_pct(&zero_den).contains("делитель 0"),
        "{}",
        solidly_fee::no_pct(&zero_den)
    );
    assert!(
        solidly_fee::no_pct(&div(0)).contains("деление на ноль"),
        "{}",
        solidly_fee::no_pct(&div(0))
    );

    let c = ctx("chk").await;
    assert_eq!(checked(&c, U256::from(9999), NUM).unwrap().value, 9999);
    assert!(checked(&c, U256::from(10_000), NUM).is_err());
    assert!(checked(&c, U256::from(1), FeeRule::Divisor).is_err());
    assert_eq!(
        checked(&c, U256::from(2), FeeRule::Divisor).unwrap().value,
        2
    );

    // usdfi: isPaused() = 0, maxGasPrice() = 5 gwei на блоке записи.
    let s = load("state.json");
    let fc = &s["dex"]["usdfi"]["factoryCalls"];
    assert_eq!(fc["isPaused()"], "0");
    let max = u(&fc["maxGasPrice()"]).to::<u64>();
    assert_eq!(max, 5_000_000_000);
    let g = SolidlyGate {
        paused: false,
        max_gas_price: Some(max),
    };
    assert_eq!(solidly_fee::blocked(&g, Some(U256::from(max))), None);
    let over = solidly_fee::blocked(&g, Some(U256::from(max + 1))).unwrap();
    assert!(over.contains("maxGasPrice"), "{over}");
    let paused = SolidlyGate {
        paused: true,
        max_gas_price: None,
    };
    assert!(
        solidly_fee::blocked(&paused, None)
            .unwrap()
            .contains("isPaused")
    );
    assert_eq!(
        solidly_fee::blocked(&SolidlyGate::default(), Some(U256::MAX)),
        None
    );
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
        assert_eq!(
            (ev.0, ev.1),
            (u(&p["getReserves"][0]), u(&p["getReserves"][1])),
            "{dex}"
        );
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
    assert_eq!(
        req("usdfi"),
        vec![
            "dex.factory",
            "dex.solidly_fee_rule",
            "dex.solidly_fee_method"
        ]
    );
    set(&c, "usdfi", &[("dex.solidly_fee_rule", json!("divisor"))]).await;
    assert_eq!(
        req("usdfi"),
        vec![
            "dex.factory",
            "dex.solidly_fee_rule",
            "dex.solidly_fee_method"
        ]
    );

    set(
        &c,
        "magicfox",
        &[
            ("dex.solidly_fee_source", json!("factory_by_type")),
            ("dex.solidly_fee_rule", json!("numerator")),
        ],
    )
    .await;
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

/// (7) Записи обоих форматов: состояние пула veplus записи (fee 5 числителем) в новом виде и с прежним `"fee": 5`.
#[test]
fn pool_record_reads_both_formats() {
    let s = load("state.json");
    let (d, p) = (&s["dex"]["veplus"], &s["dex"]["veplus"]["pools"][0]);
    let m = &p["metadata"];
    let state = PoolState::Solidly {
        reserve0: u(&m["r0"]),
        reserve1: u(&m["r1"]),
        stable: m["st"].as_bool().unwrap(),
        decimals0: u(&m["dec0"]),
        decimals1: u(&m["dec1"]),
        fee: fee_of("veplus", d, p),
        gate: SolidlyGate::default(),
    };
    let mut v = serde_json::to_value(&state).unwrap();
    let fee = v["Solidly"]["fee"].clone();
    assert_eq!(
        fee["value"],
        u(&p["fee"]).to::<u32>(),
        "запись — новый вид: {v}"
    );
    assert!(fee["rule"].is_object(), "{v}");
    assert_eq!(
        serde_json::from_value::<PoolState>(v.clone()).unwrap(),
        state
    );
    // Прежняя запись: число — числитель над fee_denominator().
    v["Solidly"]["fee"] = json!(u(&p["fee"]).to::<u32>());
    v["Solidly"].as_object_mut().unwrap().remove("gate");
    assert_eq!(serde_json::from_value::<PoolState>(v).unwrap(), state);
    let legacy: SolidlyFee = serde_json::from_value(json!(5)).unwrap();
    let den = solidly_math::fee_denominator().to::<u32>();
    assert_eq!(
        legacy,
        SolidlyFee {
            value: 5,
            rule: FeeRule::Numerator { denominator: den }
        }
    );
    let div: SolidlyFee = serde_json::from_value(json!({"value": 333, "rule": "Divisor"})).unwrap();
    assert_eq!(
        div,
        SolidlyFee {
            value: 333,
            rule: FeeRule::Divisor
        }
    );
}

/// (8) Предел цены газа: 5 gwei записи usdfi — как есть; больше u64 (uint256 max, u64::MAX + 1) — «предела нет».
#[test]
fn max_gas_wider_than_u64_is_no_cap() {
    let s = load("state.json");
    let raw = u(&s["dex"]["usdfi"]["factoryCalls"]["maxGasPrice()"]);
    assert_eq!(max_gas(raw), Some(5_000_000_000));
    assert_eq!(max_gas(U256::from(u64::MAX)), Some(u64::MAX));
    assert_eq!(max_gas(U256::from(u64::MAX) + U256::from(1u8)), None);
    assert_eq!(max_gas(U256::MAX), None);
}
