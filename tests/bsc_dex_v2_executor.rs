//! Файл: tests/bsc_dex_v2_executor.rs
//! Что делает: решение 280 — V2-форк без роутера (dex.kind = v2_fork_executor) без сети и без моков:
//! (1) вид: spec / exec_kind::of / make — в графе, без своего пути, форк, шаг V2; required — без dex.router (у v2_fork —
//!   с ним, как раньше); classify подключает DEX с фабрикой и комиссией без роутера;
//! (2) разбор РЕАЛЬНЫХ симуляций eth_simulateV1 (узел BSC, блок 133619188, tests/data/v2_fork_executor_2026-11-15):
//!   обмен через код ArbExecutor шагом V2 с измеренной комиссией прошёл у шести DEX, выход сети = math/v2 при этой
//!   комиссии, получено = формула по резервам после sync (кроме dao_swap — у монеты выхода налог); с комиссией на 1
//!   пипс ниже — пара откатила обмен («…: K»);
//! Исполнимость графа и откат покупки пробы — tests/bsc_dex_v2_executor_graph.rs.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex (families, exec_kind, quote::{v2_pair_sim, v2_pair_quote}, check, math::v2),
//! arb::settings (настоящая SQLite во временной папке).

use alloy_primitives::U256;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use arb::core::types::PoolKind;
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::check::simulate::CallResult;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::math::v2;
use arb::venues::bsc_dex::pools::exec_kind::{self, ExecKind};
use arb::venues::bsc_dex::pools::families::{self, Proto};
use arb::venues::bsc_dex::quote::v2_pair_quote;
use arb::venues::bsc_dex::quote::v2_pair_sim::{Sim, judge};

const DATA: &str = "tests/data/v2_fork_executor_2026-11-15/pairs.json";

/// Окружение на настоящей SQLite без настроек.
async fn ctx(tag: &str) -> Ctx {
    let d = std::env::temp_dir().join(format!("arb-v2x-{tag}-{}", std::process::id()));
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

/// Вид v2_fork_executor и неизменный v2_fork: флаги, вид шага, обязательные настройки, подключение без роутера.
#[tokio::test]
async fn kind_flags_and_required() {
    let c = ctx("req").await;
    assert_eq!(
        families::spec("v2_fork_executor"),
        Some((PoolKind::V2, Proto::V2, true, false, true))
    );
    assert_eq!(
        families::spec("v2_fork"),
        Some((PoolKind::V2, Proto::V2, true, true, true))
    );
    assert_eq!(exec_kind::of("v2_fork_executor"), Some(ExecKind::V2));
    assert_eq!(exec_kind::of("v2_fork"), Some(ExecKind::V2));
    let fx = families::make("dao_swap", "v2_fork_executor").unwrap();
    let fk = families::make("biswap", "v2_fork").unwrap();
    assert!(v2_pair_quote::kind_on(fx) && !v2_pair_quote::kind_on(fk));
    for id in ["dao_swap", "biswap"] {
        let s = Scope::Dex(id.into());
        c.settings
            .set("dex.v2_fee_source", s, Some(json!("fixed")), "тест")
            .await
            .unwrap();
    }
    assert_eq!(
        families::required(&c, fx),
        vec!["dex.factory", "dex.v2_fee_pct"]
    );
    assert_eq!(
        families::required(&c, fk),
        vec!["dex.factory", "dex.router", "dex.v2_fee_pct"]
    );
    assert!(
        !exec_kind::via_executor(&c, fx),
        "без исполнителя у кошелька — сигнал"
    );
    let s = Scope::Dex("dao_swap".into());
    for (n, v) in [
        ("dex.kind", json!("v2_fork_executor")),
        ("dex.pool_enabled", json!(true)),
        (
            "dex.factory",
            json!("0x940BEb635cbEeC04720AC97FADb97205676e6aa4"),
        ),
        ("dex.v2_fee_pct", json!("0.25")),
    ] {
        c.settings.set(n, s.clone(), Some(v), "тест").await.unwrap();
    }
    let f = families::classify(&c, "dao_swap").expect("подключается без dex.router");
    assert_eq!((f.graphable, f.tradable, f.fork), (true, false, true));
}

/// Итоги одной симуляции из записи.
fn results(v: &Value) -> Vec<CallResult> {
    serde_json::from_value(v.clone()).unwrap()
}

/// Число из строки записи.
fn n(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}

/// Записанные симуляции: принятие с измеренной комиссией, выход = math/v2; на 1 пипс ниже — откат пары («K»).
#[test]
fn recorded_simulations_confirm_fees() {
    let rec: Value = serde_json::from_str(&std::fs::read_to_string(DATA).unwrap()).unwrap();
    let dex = rec["dex"].as_object().unwrap();
    assert_eq!(dex.len(), 6);
    for (id, d) in dex {
        assert_eq!(d["getPair"], d["pair"], "{id}: getPair фабрики = пара");
        let pips = d["min_fee_pips"].as_u64().unwrap() as u32;
        let amount = n(&d["amount_in"]);
        let zero_in = d["token_in"] == d["token0"];
        let ratio = |p: u32| (U256::from(1_000_000 - p), U256::from(1_000_000u32));
        let ok = results(&d["ok"]);
        let Sim::Ok {
            remote,
            received,
            at_sync,
        } = judge(&ok, zero_in, amount, ratio(pips))
        else {
            panic!("{id}: обмен с комиссией {pips} пипсов должен пройти");
        };
        // Выход сети = формула math/v2 при НАШЕЙ комиссии в процентах (dex.v2_fee_pct = пипсы / 10000).
        let pct = Decimal::from(pips) / Decimal::from(10_000u32);
        let res = |i: usize| -> (U256, U256) {
            let h = ok[i].return_data.to_vec();
            let (a, b) = (
                U256::from_be_slice(&h[0..32]),
                U256::from_be_slice(&h[32..64]),
            );
            if zero_in { (a, b) } else { (b, a) }
        };
        let (ri, ro) = res(0);
        assert_eq!(
            remote,
            v2::amount_out(amount, ri, ro, v2::fee_ratio(pct).unwrap()).unwrap(),
            "{id}"
        );
        let (si, so) = res(3);
        assert_eq!(
            (si, so),
            (ri - amount, ro),
            "{id}: sync снял вход с резерва пары"
        );
        if d["out_token_taxed"].as_bool().unwrap() {
            assert!(
                received < at_sync,
                "{id}: у монеты выхода налог — получено меньше формулы"
            );
        } else {
            assert_eq!(
                received, at_sync,
                "{id}: получено = формула по резервам после sync"
            );
        }
        let mut bad = ok[..5].to_vec();
        bad.push(serde_json::from_value(d["below_1pip_last"].clone()).unwrap());
        match judge(&bad, zero_in, amount, ratio(pips - 1)) {
            Sim::Rejected(why) => assert!(why.ends_with(": K"), "{id}: {why}"),
            s => panic!("{id}: на 1 пипс ниже ожидался откат, а {s:?}"),
        }
        assert!(
            d["below_100pips_last"].as_str().unwrap().ends_with(": K"),
            "{id}"
        );
    }
}
