//! Файл: tests/arbitrum_kyber_elastic_legacy_live.rs
//! Что делает: решение 475 — KyberSwap Elastic Legacy (v1) на Arbitrum и чтение тиков частями на записи своего узла
//! (tools/arbitrum_record/kyber_elastic_legacy.py):
//! 1. тики частями (pools/elastic_ticks.rs merge по записанным ответам getTicksInRange(pool, start, length)) — части
//!   идут подряд (каждая следующая — от последнего тика прошлой), список доходит до MAX_TICK и совпадает с ответом
//!   «всё сразу» (length = 0), где узел его дал (у малого tickDistance — «out of gas», решение 462);
//! 2. фабрика v1: getPool(token0, token1, swapFeeUnits) = пул и factory() пула = фабрика v1 (проверка пула бота —
//!   pools/verify_calls.rs);
//! 3. наш выход Elastic v1 (math/swap_elastic, v1 = true) на тиках из частей = returnedAmount QuoterV2 v1.
//!
//! Кто вызывает: cargo test -- --ignored (после записи).
//! От чего зависит: tests/data/arbitrum_kyber_elastic_legacy_<дата>/, arb::venues::bsc_dex::{math::swap_elastic,
//! pools::{abi_elastic, elastic_ticks, ticks}, node::abi_dex}.

use std::collections::BTreeMap;
use std::path::PathBuf;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, U256, hex};
use serde_json::Value;

use arb::venues::bsc_dex::math::swap_elastic::{self, ElasticState};
use arb::venues::bsc_dex::math::tick_math::{MAX_TICK, MIN_TICK};
use arb::venues::bsc_dex::node::abi_dex::IElasticQuoterV2;
use arb::venues::bsc_dex::pools::abi_elastic::{IElasticFactory, IElasticPool, ITicksFeesReader};
use arb::venues::bsc_dex::pools::elastic_ticks::{Next, merge};
use arb::venues::bsc_dex::pools::load_elastic::parse_ticks;
use arb::venues::bsc_dex::pools::ticks::{TickData, TickMap};

const SKIP: &str = "нет записи: tools/arbitrum_record/kyber_elastic_legacy.py";

/// Последняя по имени папка записи. Паника — записи нет.
fn data_dir() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
        .expect("tests/data")
        .filter_map(|d| d.ok().map(|d| d.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("arbitrum_kyber_elastic_legacy_"))
        })
        .collect();
    dirs.sort();
    dirs.pop().expect(SKIP)
}

/// JSON-файл записи. Паника — нет файла или не JSON.
fn read(name: &str) -> Value {
    let text = std::fs::read_to_string(data_dir().join(name)).expect(name);
    serde_json::from_str(&text).expect("json")
}

/// Байты успешного ответа {"ok": "0x…"}; None — откат/ошибка узла.
fn ok(v: &Value) -> Option<Vec<u8>> {
    v.get("ok")
        .and_then(|h| h.as_str())
        .map(|h| hex::decode(h).expect("hex"))
}

/// Разбор успешного ответа по ABI вызова `C`. Паника — откат или ответ не по ABI.
fn ret<C: SolCall>(v: &Value) -> C::Return {
    C::abi_decode_returns(&ok(v).expect("ответ")).expect("ABI")
}

/// Пулы записи без ошибки чтения.
fn pools() -> Vec<Value> {
    let all: Vec<Value> = serde_json::from_value(read("pools.json")).expect("pools.json");
    all.into_iter()
        .filter(|p| p.get("error").is_none())
        .collect()
}

/// Тики пула по записанным частям — тем же merge, что у бота. Паника — часть откатилась или список не дочитан.
fn chunked(p: &Value) -> Vec<i32> {
    let mut out = Vec::new();
    let mut start = MIN_TICK;
    for part in p["ticks_chunks"].as_array().expect("ticks_chunks") {
        assert_eq!(part["start"].as_i64(), Some(i64::from(start)), "пул {}", p["pool"]);
        let len = part["length"].as_u64().unwrap() as u32;
        let got = ret::<ITicksFeesReader::getTicksInRangeCall>(part);
        match merge(&mut out, start, &got, len).unwrap() {
            Next::From(t) => start = t,
            Next::Done => return out,
        }
    }
    panic!("пул {}: части кончились до MAX_TICK", p["pool"]);
}

#[test]
#[ignore = "нет записи: tools/arbitrum_record/kyber_elastic_legacy.py"]
fn ticks_by_chunks_match_whole() {
    let pools = pools();
    assert!(!pools.is_empty(), "в записи нет пулов v1");
    for p in &pools {
        let ticks = chunked(p);
        assert_eq!(ticks.first(), Some(&MIN_TICK));
        assert_eq!(ticks.last(), Some(&MAX_TICK));
        if ok(&p["getTicksInRange"]).is_some() {
            let all = ret::<ITicksFeesReader::getTicksInRangeCall>(&p["getTicksInRange"]);
            assert_eq!(ticks, parse_ticks(&all), "пул {}", p["pool"]);
        }
    }
}

#[test]
#[ignore = "нет записи: tools/arbitrum_record/kyber_elastic_legacy.py"]
fn legacy_factory_confirms_pool() {
    let factory: Address = read("meta.json")["v1"]["factory"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    for p in pools() {
        let pool: Address = p["pool"].as_str().unwrap().parse().unwrap();
        assert_eq!(ret::<IElasticFactory::getPoolCall>(&p["factory_getPool"]), pool);
        let f = Address::from_slice(&ok(&p["factory"]).expect("factory()")[12..]);
        assert_eq!(f, factory, "пул {pool}");
    }
}

#[test]
#[ignore = "нет записи: tools/arbitrum_record/kyber_elastic_legacy.py"]
fn legacy_math_matches_quoter() {
    let mut checked = 0;
    for p in pools() {
        let st = ret::<IElasticPool::getPoolStateCall>(&p["getPoolState"]);
        let liq = ret::<IElasticPool::getLiquidityStateCall>(&p["getLiquidityState"]);
        let fee = ret::<IElasticPool::swapFeeUnitsCall>(&p["swapFeeUnits"]);
        let dist = ret::<IElasticPool::tickDistanceCall>(&p["tickDistance"]);
        let mut map = BTreeMap::new();
        for t in chunked(&p) {
            let i = ret::<IElasticPool::ticksCall>(&p["ticks"][t.to_string()]);
            if i.liquidityGross > 0 {
                let d = TickData {
                    gross: i.liquidityGross,
                    net: i.liquidityNet,
                };
                map.insert(t, d);
            }
        }
        let tm = TickMap {
            spacing: dist.as_i32(),
            ticks: map.into(),
            words: (i16::MIN, i16::MAX),
        };
        let s = ElasticState {
            sqrt_p: U256::from(st.sqrtP),
            current_tick: st.currentTick.as_i32(),
            base_l: liq.baseL,
            reinvest_l: liq.reinvestL,
            fee_units: fee.to(),
            ticks: &tm,
            v1: true,
        };
        let t0 = Address::from_slice(&ok(&p["token0"]).expect("token0")[12..]);
        for q in p["quotes"].as_array().unwrap() {
            if ok(q).is_none() {
                continue;
            }
            let out = ret::<IElasticQuoterV2::quoteExactInputSingleCall>(q);
            let token_in: Address = q["tokenIn"].as_str().unwrap().parse().unwrap();
            let amount: U256 = q["amountIn"].as_str().unwrap().parse().unwrap();
            let ours = swap_elastic::swap(&s, token_in == t0, true, amount).unwrap();
            assert_eq!(
                (ours.amount_in, ours.amount_out),
                (out.usedAmount, out.returnedAmount),
                "пул {} вход {token_in} сумма {amount}",
                p["pool"]
            );
            checked += 1;
        }
    }
    assert!(checked > 0, "ни одного ответа квотера в записи");
}
