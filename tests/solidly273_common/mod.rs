//! Файл: tests/solidly273_common/mod.rs
//! Что делает: решение 273 — общее для тестов комиссии Solidly (tests/bsc_dex_solidly_fee_273.rs,
//! tests/bsc_dex_solidly_fee_273_cfg.rs): чтение записи узла tests/data/solidly_fee_2026-11-14, комиссия пула записи
//! по правилу его DEX, выход пары путём quote/pool_quote.rs, образцы getAmountOut, окружение на настоящей SQLite.
//! Кто вызывает: tests/bsc_dex_solidly_fee_273*.rs.
//! От чего зависит: arb::venues::bsc_dex (math::{solidly_fee, solidly_v1_math}, ctx), arb::core::types, arb::settings,
//! arb::storage. Моков нет.
#![allow(dead_code)]

use alloy_primitives::U256;
use serde_json::Value;
use tokio::sync::broadcast;

use arb::core::types::{FeeRule, SolidlyFee};
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::math::{solidly_fee, solidly_v1_math};

pub const DATA: &str = "tests/data/solidly_fee_2026-11-14";
pub const NUM: FeeRule = FeeRule::Numerator {
    denominator: arb::core::types::pool_solidly::GETFEE_DENOMINATOR,
};

pub fn load(name: &str) -> Value {
    let raw = std::fs::read_to_string(format!("{DATA}/{name}")).unwrap();
    serde_json::from_str(&raw).unwrap()
}

pub fn u(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}

/// Комиссия пула записи по правилу его DEX: usdfi — fee() делителем, veplus — swapFee() числителем над 10000,
/// magicfox — stableFee()/volatileFee() фабрики числителем над 10000.
pub fn fee_of(dex: &str, d: &Value, p: &Value) -> SolidlyFee {
    let stable = p["metadata"]["st"].as_bool().unwrap();
    let (raw, rule) = match dex {
        "usdfi" => (u(&p["fee"]), FeeRule::Divisor),
        "veplus" => (u(&p["fee"]), NUM),
        _ => {
            let m = if stable {
                "stableFee()"
            } else {
                "volatileFee()"
            };
            (u(&d["factoryCalls"][m]), NUM)
        }
    };
    SolidlyFee {
        value: raw.to::<u32>(),
        rule,
    }
}

/// Выход пары путём quote/pool_quote.rs: вход после комиссии по правилу, формула пары с нулевой комиссией.
pub fn ours(fee: &SolidlyFee, p: &Value, amount: U256, token_in: &str) -> Option<U256> {
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
pub fn samples(s: &Value) -> Vec<(String, Value, Value, U256, String, U256)> {
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

/// Окружение на настоящей SQLite без настроек.
pub async fn ctx(tag: &str) -> Ctx {
    let d = std::env::temp_dir().join(format!("arb-sol273-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db, changes, journal.clone()).await.unwrap();
    {
        let c = Ctx {
            raw: RawLog::start(d.join("raw"), settings.clone()),
            pace: Default::default(),
            failover: Default::default(),
            evm_chain: Default::default(),
            place: arb::venues::bsc_dex::ctx::Place::BSC,
            chains: Default::default(),
            account: None,
            index: Default::default(),
            settings,
            vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
            http: reqwest::Client::new(),
            journal,
        };
        // Решение 416: тикер газа площадки DEX — настройка (dex.gas_ticker).
        c.settings
            .set(
                "dex.gas_ticker",
                arb::settings::scope::Scope::Venue("bsc_dex".into()),
                Some(serde_json::json!("BNB")),
                "тест",
            )
            .await
            .unwrap();
        c
    }
}

/// Задаёт настройки в области DEX id.
pub async fn set(c: &Ctx, id: &str, list: &[(&str, Value)]) {
    for (n, v) in list {
        c.settings
            .set(n, Scope::Dex(id.into()), Some(v.clone()), "тест")
            .await
            .unwrap();
    }
}
