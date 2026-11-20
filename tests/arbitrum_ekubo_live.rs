//! Файл: tests/arbitrum_ekubo_live.rs
//! Что делает: решение 447 (Ekubo, T433-F8 / T433-079) — проверки на записи своего узла Arbitrum
//! (tests/data/arbitrum_ekubo_<дата>/, пишет tools/arbitrum_record/ekubo.py; берётся последняя по имени папка):
//! 1. ключ пула из PoolKeyIndex.poolKeyById даёт тот же PoolId (abi_ekubo::pool_id) — как проверка verify_ekubo.rs;
//! 2. наш расчёт обмена (math/ekubo_swap.rs по состоянию, собранному как в pools/load_ekubo.rs, + доплата MEVCapture)
//!    = выход MEVCaptureRouter.quote на том же блоке (quote/quoter_ekubo.rs decode), каждый полный расчёт;
//! 3. логи ядра за блоки записи: у анонимного лога обмена (log0, 116 байт) и у PositionUpdated PoolId берётся из
//!    данных (pools/ekubo_route.rs), stateAfter разбирается (abi_ekubo::decode_state).
//!
//! Записи пока нет (у бота нет доступа к узлу Arbitrum) — тесты помечены #[ignore]; после записи метку снимаем.
//! Кто вызывает: cargo test -- --ignored.
//! От чего зависит: arb::venues::bsc_dex::{math, pools, quote}, файлы записи. Моков нет.

use alloy::rpc::types::Log;
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, U256};
use serde_json::Value;

use arb::core::types::{EkuboData, EkuboTick};
use arb::venues::bsc_dex::math::{ekubo_config, ekubo_mev, ekubo_sqrt, ekubo_swap};
use arb::venues::bsc_dex::pools::abi_ekubo::{self, IEkuboKeyIndex};
use arb::venues::bsc_dex::pools::{ekubo_route, load_ekubo};
use arb::venues::bsc_dex::quote::quoter_ekubo;

const SKIP: &str = "нет записи: tools/arbitrum_record/ekubo.py";

/// Папка последней записи. Ошибки: паника — записи нет.
fn dir() -> std::path::PathBuf {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data");
    let mut found: Vec<_> = std::fs::read_dir(&root)
        .expect("tests/data")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("arbitrum_ekubo_"))
        })
        .collect();
    found.sort();
    found.pop().expect(SKIP)
}

/// JSON-файл записи. Ошибки: паника — файла нет или он не JSON.
fn json(name: &str) -> Value {
    let text = std::fs::read_to_string(dir().join(name)).expect(name);
    serde_json::from_str(&text).expect(name)
}

/// Байты из «0x…». Ошибки: паника — не hex.
fn hex(v: &Value) -> Vec<u8> {
    let s = v.as_str().expect("hex-строка");
    alloy_primitives::hex::decode(s).expect("hex")
}

/// Слово из «0x…» (32 байта). Ошибки: паника.
fn word(v: &Value) -> B256 {
    B256::from_slice(&hex(v))
}

/// Ключ пула из ответа poolKeyById. None — пула нет в реестре. Ошибки: паника — ответ не по ABI.
fn key(rec: &Value) -> Option<(Address, Address, B256)> {
    let raw = hex(&rec["poolKeyById"]);
    let k = IEkuboKeyIndex::poolKeyByIdCall::abi_decode_returns(&raw).expect("poolKeyById");
    (k.token1 != Address::ZERO).then_some((k.token0, k.token1, k.config))
}

/// Состояние пула из записи — как pools/load_ekubo.rs (те же разборщики). Ошибок нет (паника — запись не та).
fn state(rec: &Value, config: B256, block_ts: u64) -> EkuboData {
    let (sqrt_ratio, tick, liquidity) = abi_ekubo::decode_state(&word(&rec["state"]));
    let words = rec["words"]
        .as_array()
        .map_or((0, 0), |w| (w[0].as_u64().unwrap(), w[1].as_u64().unwrap()));
    let bitmap: Vec<(u64, U256)> = rec["bitmap"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|e| (e["word"].as_u64().unwrap(), U256::from_be_bytes(word(&e["bits"]).0)))
                .collect()
        })
        .unwrap_or_default();
    let ticks: Vec<EkuboTick> = rec["ticks"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|e| {
                    let (net, delta) = abi_ekubo::decode_tick(&word(&e["info"]));
                    let tick = e["tick"].as_i64().unwrap() as i32;
                    EkuboTick { tick, net, delta }
                })
                .collect()
        })
        .unwrap_or_default();
    if let ekubo_config::Shape::Concentrated { spacing } = ekubo_config::shape(&config) {
        let set = load_ekubo::set_ticks(&bitmap, spacing);
        let have: Vec<i32> = ticks.iter().map(|t| t.tick).collect();
        assert_eq!(set, have, "тики записи = установленные биты битмапа");
    }
    let mev = rec.get("mev_state").map(|v| load_ekubo::mev_state(&word(v)));
    EkuboData {
        config,
        sqrt_ratio,
        tick,
        liquidity,
        words,
        bitmap,
        ticks,
        mev,
        block_ts,
        native: None,
        twamm: None,
    }
}

#[test]
#[ignore = "нет записи: tools/arbitrum_record/ekubo.py"]
fn key_gives_pool_id() {
    let mut n = 0;
    for rec in json("pools.json").as_array().unwrap() {
        if rec.get("poolKeyById").is_none() {
            continue;
        }
        let Some((t0, t1, config)) = key(rec) else {
            continue;
        };
        let id = word(&rec["pool_id"]);
        assert_eq!(abi_ekubo::pool_id(t0, t1, config), id);
        n += 1;
    }
    assert!(n > 0, "в записи нет пулов из PoolKeyIndex");
}

#[test]
#[ignore = "нет записи: tools/arbitrum_record/ekubo.py"]
fn math_equals_router_quote() {
    let ts = json("meta.json")["timestamp"].as_u64().unwrap();
    let mut compared = 0;
    for rec in json("pools.json").as_array().unwrap() {
        let Some(quotes) = rec["quotes"].as_array() else {
            continue;
        };
        let Some((_, _, config)) = key(rec) else {
            continue;
        };
        let d = state(rec, config, ts);
        for q in quotes {
            let Some(out) = q.get("out") else {
                continue;
            };
            let router = quoter_ekubo::decode(&hex(out));
            let is_token1 = q["is_token1"].as_bool().unwrap();
            let amount: u128 = q["amount"].as_str().unwrap().parse().unwrap();
            let Some(ours) = ekubo_swap::exact_in(&d, is_token1, amount) else {
                assert!(router.is_none(), "наш расчёт — откат, роутер ответил {router:?}");
                continue;
            };
            if !ours.complete || ours.amount_in != amount {
                continue;
            }
            // MEVCapture: tickLast — тик до обмена (новая секунда) или сохранённый (та же секунда, что блок записи).
            let mut can = vec![ours.amount_out];
            if let Some(m) = &d.mev {
                can = [d.tick, m.tick_last]
                    .into_iter()
                    .map(|last| {
                        let fee = ekubo_mev::additional_fee(&d, ours.tick_after, last);
                        ours.amount_out - ekubo_sqrt::compute_fee(ours.amount_out, fee)
                    })
                    .collect();
            }
            let got = router.map(|v| v.to::<u128>());
            assert!(
                got.is_some_and(|g| can.contains(&g)),
                "пул {}: вход {amount} (token1: {is_token1}) — роутер {got:?}, наш {can:?}",
                rec["pool_id"]
            );
            compared += 1;
        }
    }
    assert!(compared > 0, "ни одного полного расчёта для сверки");
}

#[test]
#[ignore = "нет записи: tools/arbitrum_record/ekubo.py"]
fn core_logs_route() {
    let all = json("core_logs.json");
    let mut swaps = 0;
    for raw in all["logs"].as_array().unwrap() {
        let log: Log = serde_json::from_value(raw.clone()).expect("лог");
        let data = &log.inner.data.data[..];
        if log.topics().is_empty() {
            let (id, _, st) = abi_ekubo::decode_swap_log(data).expect("log0 обмена — 116 байт");
            assert_eq!(ekubo_route::pool_id(&log), Some(id));
            let (sqrt, _, _) = abi_ekubo::decode_state(&st);
            assert!(!sqrt.is_zero(), "stateAfter обмена с нулевой ценой");
            swaps += 1;
        }
    }
    assert!(swaps > 0, "в записи нет обменов ядра");
}
