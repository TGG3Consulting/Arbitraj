//! Файл: tests/arbitrum_kyber_elastic_v1_live.rs
//! Что делает: решение 451 (T448-06) — сверка математики KyberSwap Elastic v1 (math/swap_elastic, ElasticState.v1 =
//! true) с QuoterV2 развёртывания Arbitrum на записи своего узла: на одном блоке состояние пула (getPoolState,
//! getLiquidityState, swapFeeUnits), тики (TicksFeesReader.getTicksInRange + ticks(t)) и ответы
//! quoteExactInputSingle на суммы в обе стороны. Наш выход должен совпасть с returnedAmount квотера побайтно
//! (квотер вызывает тот же Pool.swap в try/catch — ks-elastic-sc QuoterV2.sol). Плюс: getPool фабрики = пул.
//! Кто вызывает: cargo test -- --ignored (после записи).
//! От чего зависит: tests/data/arbitrum_kyber_elastic_v1_<дата>/ (tools/arbitrum_record/kyber_elastic_v1.py),
//! arb::venues::bsc_dex::{math::swap_elastic, pools::{abi_elastic, load_elastic, ticks}, node::abi_dex}.

use std::collections::BTreeMap;
use std::path::PathBuf;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, U256, hex};
use serde_json::Value;

use arb::venues::bsc_dex::math::swap_elastic::{self, ElasticState};
use arb::venues::bsc_dex::node::abi_dex::IElasticQuoterV2;
use arb::venues::bsc_dex::pools::abi_elastic::{IElasticFactory, IElasticPool, ITicksFeesReader};
use arb::venues::bsc_dex::pools::load_elastic::parse_ticks;
use arb::venues::bsc_dex::pools::ticks::{TickData, TickMap};

/// Последняя по имени папка записи tests/data/arbitrum_kyber_elastic_v1_*. Паника — записи нет.
fn data_dir() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
        .expect("tests/data")
        .filter_map(|d| d.ok().map(|d| d.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("arbitrum_kyber_elastic_v1_"))
        })
        .collect();
    dirs.sort();
    dirs.pop().expect("нет записи: tools/arbitrum_record/kyber_elastic_v1.py")
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

/// Пулы записи с полным состоянием.
fn pools() -> Vec<Value> {
    let text = std::fs::read_to_string(data_dir().join("pools.json")).expect("pools.json");
    let all: Vec<Value> = serde_json::from_str(&text).expect("json");
    all.into_iter()
        .filter(|p| p.get("error").is_none() && ok(&p["getTicksInRange"]).is_some())
        .collect()
}

/// Наш выход Elastic v1 = returnedAmount QuoterV2 на каждой записанной сумме (откат квотера — пропуск).
#[test]
#[ignore = "запись 2026-11-20 не о v1: у всех 20 пулов GeckoTerminal factory() = 0xc7a590291e07…, не фабрика v1 0x5F1dddbf…; getPool v1 ≠ пул — повтор kyber_elastic_v1.py с отбором пулов фабрики v1 (TASKS T456-03)"]
fn v1_math_matches_quoter() {
    let pools = pools();
    assert!(!pools.is_empty(), "в записи нет пулов с тиками");
    let mut checked = 0;
    for p in &pools {
        let st = ret::<IElasticPool::getPoolStateCall>(&p["getPoolState"]);
        let liq = ret::<IElasticPool::getLiquidityStateCall>(&p["getLiquidityState"]);
        let fee = ret::<IElasticPool::swapFeeUnitsCall>(&p["swapFeeUnits"]);
        let dist = ret::<IElasticPool::tickDistanceCall>(&p["tickDistance"]);
        let all = ret::<ITicksFeesReader::getTicksInRangeCall>(&p["getTicksInRange"]);
        let mut map = BTreeMap::new();
        for t in parse_ticks(&all) {
            let i = ret::<IElasticPool::ticksCall>(&p["ticks"][t.to_string()]);
            if i.liquidityGross > 0 {
                map.insert(
                    t,
                    TickData {
                        gross: i.liquidityGross,
                        net: i.liquidityNet,
                    },
                );
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

/// Фабрика v1: getPool(token0, token1, swapFeeUnits) = адрес пула (проверка пула pools/verify_calls.rs).
#[test]
#[ignore = "запись 2026-11-20 не о v1: у всех 20 пулов GeckoTerminal factory() = 0xc7a590291e07…, не фабрика v1 0x5F1dddbf…; getPool v1 ≠ пул — повтор kyber_elastic_v1.py с отбором пулов фабрики v1 (TASKS T456-03)"]
fn v1_factory_confirms_pool() {
    for p in pools() {
        let got = ret::<IElasticFactory::getPoolCall>(&p["factory_getPool"]);
        let pool: Address = p["pool"].as_str().unwrap().parse().unwrap();
        assert_eq!(got, pool);
    }
}
