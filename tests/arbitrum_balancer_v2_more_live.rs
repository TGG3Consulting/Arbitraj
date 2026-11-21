//! Файл: tests/arbitrum_balancer_v2_more_live.rs
//! Что проверяет: решение 473 (T450-03…07) на РЕАЛЬНЫХ ответах своего узла Arbitrum One, записанных
//! tools/arbitrum_record/balancer_v2_more.py на одном блоке (tests/data/arbitrum_balancer_v2_more_<дата>/; записи пока
//! нет — тесты под #[ignore], после записи метку снять).
//! 1. Пачка блока бота (load_balancer_more::calls) по неизменяемому из записи — те же вызовы (адрес и calldata), что
//!    записал скрипт; неизменяемое согласовано (load_balancer_more_fixed::check).
//! 2. Выход math/balancer_math.rs (ветка VaultCurve::V2 / весовая) на состоянии разбора load_balancer::decode = −delta
//!    выхода Vault.queryBatchSwap GIVEN_IN той же суммы (откат хранилища — у нас None); пара с BPT пула у composable
//!    stable / stable phantom / managed — разбор отказывает.
//! 3. gt.json: строки GeckoTerminal balancer_arbitrum с фабрикой — сумма reserve_in_usd по видам (печать: порядок
//!    ликвидности).
//!
//! Кто вызывает: cargo test -- --ignored (после записи).
//! От чего зависит: arb::venues::bsc_dex::{math::balancer_math, pools::{abi_balancer, abi_balancer_more,
//! balancer_pool_type, load_balancer, load_balancer_fixed, load_balancer_more, load_balancer_more_fixed}}. Моков нет.

use std::collections::HashMap;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, U256};
use serde_json::Value;

use arb::core::types::PoolState;
use arb::venues::bsc_dex::math::balancer_math;
use arb::venues::bsc_dex::pools::abi_balancer::IBalancerVault as V;
use arb::venues::bsc_dex::pools::abi_balancer_more::{
    IComposable as C, ILinear as L, IMetaStable as M,
};
use arb::venues::bsc_dex::pools::balancer_pool_type::{PoolType, parse};
use arb::venues::bsc_dex::pools::load_balancer_fixed::Fixed;
use arb::venues::bsc_dex::pools::load_balancer_more_fixed::{More, check};
use arb::venues::bsc_dex::pools::{load_balancer, load_balancer_more};

const NO_DATA: &str = "нет записи: tools/arbitrum_record/balancer_v2_more.py";

/// Папка последней записи (по имени). Паника — записи нет.
fn dir() -> std::path::PathBuf {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data");
    let mut all: Vec<_> = std::fs::read_dir(&root)
        .expect("tests/data")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("arbitrum_balancer_v2_more_"))
        })
        .collect();
    all.sort();
    all.pop().expect(NO_DATA)
}

fn json(name: &str) -> Value {
    let p = dir().join(name);
    serde_json::from_str(&std::fs::read_to_string(p).expect(NO_DATA)).unwrap()
}

fn addr(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}

/// Ответ записанного вызова: байты или текст отката.
fn answer(v: &Value) -> Result<Bytes, String> {
    match v["result"].as_str() {
        Some(h) => Ok(Bytes::from(alloy_primitives::hex::decode(h).unwrap())),
        None => Err(v["error"].to_string()),
    }
}

fn ok(v: &Value) -> Bytes {
    answer(v).expect("вызов откатился")
}

/// Неизменяемое пула из записанных ответов — как load_balancer_more_fixed::read (без сети).
fn more(p: &Value, ty: PoolType, multicall: Address) -> (More, HashMap<Address, U256>) {
    let tokens: Vec<Address> = p["tokens"].as_array().unwrap().iter().map(addr).collect();
    let mut scaling = HashMap::new();
    let mut dec = Vec::new();
    for (t, d) in tokens.iter().zip(p["decimals"].as_array().unwrap()) {
        let d = U256::from_be_slice(&ok(d)).to::<u64>();
        let s = U256::from(10u8).pow(U256::from(18 - d));
        scaling.insert(*t, s);
        dec.push(balancer_math::one() * s);
    }
    let tf = p["type_fixed"].as_array().unwrap();
    let ix = |v: &Value| U256::from_be_slice(&ok(v)).to::<usize>();
    let pool = addr(&p["pool"]);
    let mut m = More {
        ty,
        tokens: tokens.clone(),
        dec,
        bpt: None,
        providers: Vec::new(),
        main: 0,
        wrapped: 0,
        time: Some(multicall),
    };
    match ty {
        PoolType::Composable => {
            m.bpt = Some(ix(&tf[0]));
            m.providers = C::getRateProvidersCall::abi_decode_returns(&ok(&tf[1])).unwrap();
        }
        PoolType::Phantom => m.bpt = Some(ix(&tf[0])),
        PoolType::MetaStable => {
            m.providers = M::getRateProvidersCall::abi_decode_returns(&ok(&tf[0])).unwrap();
        }
        PoolType::LinearV1 | PoolType::Linear => {
            m.main = ix(&tf[0]);
            m.wrapped = ix(&tf[1]);
            m.bpt = Some(ix(&tf[2]));
        }
        PoolType::Managed => m.bpt = tokens.iter().position(|t| *t == pool),
        PoolType::Lbp | PoolType::Investment => {}
    }
    check(&m, pool).unwrap();
    (m, scaling)
}

#[test]
#[ignore = "нет записи: tools/arbitrum_record/balancer_v2_more.py"]
fn calls_and_math_equal_query_batch_swap() {
    let b = json("block.json");
    let (vault, multicall) = (addr(&b["vault"]), addr(&b["multicall"]));
    let (mut checked, mut pools) = (0, 0);
    for p in json("pools.json").as_array().unwrap() {
        if !p["skipped"].is_null() {
            eprintln!("{}: пропущен в записи — {}", p["pool"], p["skipped"]);
            continue;
        }
        let ty = parse(p["type"].as_str().unwrap()).unwrap();
        let pool = addr(&p["pool"]);
        let id = B256::from_slice(&ok(&p["fixed"][0]));
        let (m, scaling) = more(p, ty, multicall);
        let ours = load_balancer_more::calls(vault, pool, id, &m);
        let rec = p["batch"].as_array().unwrap();
        assert_eq!(ours.len(), rec.len(), "{pool}: число вызовов пачки");
        for ((to, data), r) in ours.iter().zip(rec) {
            assert_eq!(*to, addr(&r["to"]), "{pool}: адрес вызова {}", r["name"]);
            let want = alloy_primitives::hex::decode(r["data"].as_str().unwrap()).unwrap();
            assert_eq!(data.to_vec(), want, "{pool}: calldata {}", r["name"]);
        }
        let got: Vec<Result<Bytes, String>> = rec.iter().map(answer).collect();
        let tokens = m.tokens.clone();
        let fx = Fixed {
            id,
            weighted: ty.weighted(),
            scaling,
            v2: false,
            more: Some(Box::new(m)),
        };
        pools += 1;
        for q in p["query"].as_array().unwrap() {
            let (i, j) = (
                q["i"].as_u64().unwrap() as usize,
                q["j"].as_u64().unwrap() as usize,
            );
            let amount: U256 = q["amount"].as_str().unwrap().parse().unwrap();
            let st = load_balancer::decode(&fx, &got, (tokens[i], tokens[j]));
            if ty.bpt_excluded() && (tokens[i] == pool || tokens[j] == pool) {
                assert!(st.is_err(), "{pool}: пара с BPT пула должна отказывать");
                continue;
            }
            let PoolState::VaultPool(d) = st.unwrap() else {
                panic!("{pool}: не VaultPool");
            };
            let ours = balancer_math::amount_out(&d, i, j, amount);
            match answer(q) {
                Ok(raw) => {
                    let deltas = V::queryBatchSwapCall::abi_decode_returns(&raw).unwrap();
                    assert!(
                        deltas[j].is_negative(),
                        "{pool}: дельта выхода не отрицательна"
                    );
                    let want = deltas[j].unsigned_abs();
                    assert_eq!(ours, Some(want), "{pool} {ty:?} {i}→{j} {amount}");
                }
                Err(_) => {
                    assert_eq!(
                        ours, None,
                        "{pool} {ty:?} {i}→{j} {amount}: хранилище откатило"
                    )
                }
            }
            checked += 1;
        }
    }
    eprintln!("пулов {pools}, сверок {checked}");
    assert!(checked > 0, "в записи нет ни одной сверки");
}

#[test]
#[ignore = "нет записи: tools/arbitrum_record/balancer_v2_more.py"]
fn liquidity_by_type() {
    let mut sum: HashMap<String, f64> = HashMap::new();
    let rows = json("gt.json");
    for r in rows.as_array().unwrap() {
        let usd: f64 = r["reserve_usd"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0);
        let ty = r["factory"]
            .as_str()
            .and_then(|f| f.split(':').nth(1))
            .unwrap_or("другие фабрики (весовые / стейблсвап решений 289, 450)");
        *sum.entry(ty.to_string()).or_default() += usd;
    }
    let mut list: Vec<_> = sum.into_iter().collect();
    list.sort_by(|a, b| b.1.total_cmp(&a.1));
    for (ty, usd) in &list {
        eprintln!("{ty}: {usd:.0} USD");
    }
    assert!(!list.is_empty(), "gt.json пуст");
}

/// Ответ getTargets / getWrappedTokenRate разбирается тем же ABI, что у бота (проверка записи linear-пулов).
#[test]
#[ignore = "нет записи: tools/arbitrum_record/balancer_v2_more.py"]
fn linear_targets_ordered() {
    for p in json("pools.json").as_array().unwrap() {
        let lin = matches!(p["type"].as_str(), Some("linear") | Some("linear_v1"));
        if !lin || !p["skipped"].is_null() {
            continue;
        }
        let t = L::getTargetsCall::abi_decode_returns(&ok(&p["batch"][3])).unwrap();
        assert!(
            t.lowerTarget <= t.upperTarget,
            "{}: нижняя цель выше верхней",
            p["pool"]
        );
    }
}
