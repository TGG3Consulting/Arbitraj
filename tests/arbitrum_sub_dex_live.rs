//! Файл: tests/arbitrum_sub_dex_live.rs
//! Что проверяет: решение 450 (T448-03, T448-04, T448-05) на РЕАЛЬНЫХ ответах своего узла Arbitrum One, записанных
//! tools/arbitrum_record/sub_dex.py на одном блоке (tests/data/arbitrum_sub_dex_<дата>/; записи пока нет — тесты под
//! #[ignore], после записи метку снять).
//! 1. KyberSwap Classic: событие создания пула каждой из трёх фабрик (DMM, KSFactory, KSFactory Legacy) разбирается
//!    abi_dmm::created в (token0, token1, пул) события; factory() пула = фабрика, getPools фабрики содержит пул; выход
//!    math/dmm.rs по getTradeInfo блока = ответ getAmountsOut роутера своей фабрики (откат роутера — у нас None).
//! 2. Balancer V2: выход math/balancer_math.rs (кривая по версии фабрики из записи: v1 — Weighted / Stable, v2 —
//!    WeightedV2 / StableV2) на записанном состоянии = −delta выхода Vault.queryBatchSwap GIVEN_IN (откат — None).
//! 3. iZiSwap: ответ реестра pool(x, y, fee) в обоих порядках разбирается factory_ask::decode_q(Q::Izi) в адрес пула
//!    события; fee2pointDelta уровней записи ≠ 0.
//!
//! Кто вызывает: cargo test -- --ignored (после записи).
//! От чего зависит: arb::venues::bsc_dex::{math::{dmm, balancer_math}, pools::{abi_dmm, factory_ask, load_balancer}}.

use alloy_primitives::{Address, B256, Bytes, I256, LogData, U256};
use serde_json::Value;

use arb::core::types::{PoolState, VaultCurve};
use arb::venues::bsc_dex::math::{balancer_math, dmm};
use arb::venues::bsc_dex::pools::factory_ask::{Q, decode_q};
use arb::venues::bsc_dex::pools::{abi_dmm, load_balancer};

const NO_DATA: &str = "нет записи: tools/arbitrum_record/sub_dex.py";

/// Папка последней записи tests/data/arbitrum_sub_dex_<дата>/ (по имени). Паника — записи нет.
fn dir() -> std::path::PathBuf {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data");
    let mut all: Vec<_> = std::fs::read_dir(&root)
        .expect("tests/data")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("arbitrum_sub_dex_"))
        })
        .collect();
    all.sort();
    all.pop().expect(NO_DATA)
}

fn json(name: &str) -> Value {
    let p = dir().join(name);
    serde_json::from_str(&std::fs::read_to_string(p).expect(NO_DATA)).unwrap()
}

fn bytes(v: &Value) -> Vec<u8> {
    alloy_primitives::hex::decode(v.as_str().unwrap()).unwrap()
}

/// Слова ответа eth_call (32 байта).
fn words(v: &Value) -> Vec<U256> {
    bytes(v).chunks(32).map(U256::from_be_slice).collect()
}

fn addr(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}

fn word_addr(w: U256) -> Address {
    Address::from_slice(&w.to_be_bytes::<32>()[12..])
}

/// Динамический массив слов ответа: смещение — в слове `head`.
fn dyn_list(w: &[U256], head: usize) -> Vec<U256> {
    let off = w[head].to::<usize>() / 32;
    let n = w[off].to::<usize>();
    w[off + 1..off + 1 + n].to_vec()
}

#[test]
#[ignore = "запись 2026-11-20 пуста: событий создания пулов за окно 2 000 000 блоков нет (logs_total 0 у всех фабрик) — повтор sub_dex.py с большим --blocks (TASKS T456-03)"]
fn kyber_classic_created_and_math() {
    let k = json("kyber.json");
    let mut checked = 0;
    for f in k["factories"].as_array().unwrap() {
        let factory = addr(&f["factory"]);
        for p in f["pools"].as_array().unwrap() {
            let pool = addr(&p["pool"]);
            let lg = &p["log"];
            let topics: Vec<B256> = lg["topics"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t.as_str().unwrap().parse().unwrap())
                .collect();
            let data = LogData::new(topics.clone(), Bytes::from(bytes(&lg["data"]))).unwrap();
            let (t0, t1, got) = abi_dmm::created(topics[0], &data).unwrap();
            assert_eq!((t0, t1, got), (addr(&p["token0"]), addr(&p["token1"]), pool));
            assert_eq!(word_addr(words(&p["factory"]["ok"])[0]), factory, "factory() пула");
            let reg = dyn_list(&words(&p["getPools"]["ok"]), 0);
            assert!(reg.iter().any(|w| word_addr(*w) == pool), "getPools содержит пул");
            let ti = words(&p["getTradeInfo"]["ok"]);
            for q in p["getAmountsOut"].as_array().unwrap() {
                let zero_in = addr(&q["token_in"]) == t0;
                let (r_in, r_out, v_in, v_out) = if zero_in {
                    (ti[0], ti[1], ti[2], ti[3])
                } else {
                    (ti[1], ti[0], ti[3], ti[2])
                };
                let side = dmm::Side {
                    r_in,
                    r_out,
                    v_in,
                    v_out,
                };
                let amount = U256::from_str_radix(q["amount_in"].as_str().unwrap(), 10).unwrap();
                let ours = dmm::amount_out(amount, side, ti[4]);
                match q["answer"].get("ok") {
                    Some(a) => {
                        let out = dyn_list(&words(a), 0);
                        assert_eq!(ours, Some(out[1]), "пул {pool} вход {amount}");
                    }
                    None => assert_eq!(ours, None, "роутер откатил — пул {pool} вход {amount}"),
                }
                checked += 1;
            }
        }
    }
    assert!(checked > 0, "в записи нет ответов роутеров");
}

#[test]
#[ignore = "запись 2026-11-20 пуста: событий создания пулов за окно 2 000 000 блоков нет (logs_total 0 у всех фабрик) — повтор sub_dex.py с большим --blocks (TASKS T456-03)"]
fn balancer_v2_versions_match_vault() {
    let b = json("balancer.json");
    let mut checked = 0;
    for f in b["factories"].as_array().unwrap() {
        let v2 = f["math"] == "v2";
        let name = f["name"].as_str().unwrap();
        for p in f["pools"].as_array().unwrap() {
            let own = &p["isPoolFromFactory"][name]["ok"];
            if own.is_null() || words(own)[0].is_zero() || p["queryBatchSwap"].is_null() {
                continue;
            }
            let pt = words(&p["getPoolTokens"]["ok"]);
            let tokens: Vec<Address> = dyn_list(&pt, 0).into_iter().map(word_addr).collect();
            let balances = dyn_list(&pt, 1);
            let scaling: Vec<U256> = tokens
                .iter()
                .map(|t| {
                    let d = words(&p["decimals"][format!("{t:#x}")]["ok"])[0];
                    U256::from(10u8).pow(U256::from(18u8) - d)
                })
                .collect();
            let fee = words(&p["getSwapFeePercentage"]["ok"])[0];
            let curve = match p["getNormalizedWeights"].get("ok") {
                Some(w) => {
                    let weights = dyn_list(&words(w), 0);
                    if v2 {
                        VaultCurve::WeightedV2 { weights }
                    } else {
                        VaultCurve::Weighted { weights }
                    }
                }
                None => {
                    let a = words(&p["getAmplificationParameter"]["ok"]);
                    assert_eq!(a[2], U256::from(1000u32), "точность A");
                    if v2 {
                        VaultCurve::StableV2 { amp: a[0] }
                    } else {
                        VaultCurve::Stable { amp: a[0] }
                    }
                }
            };
            let id: B256 = p["getPoolId"]["ok"].as_str().unwrap().parse().unwrap();
            for q in p["queryBatchSwap"].as_array().unwrap() {
                let (i, j) = (q["i"].as_u64().unwrap() as usize, q["j"].as_u64().unwrap() as usize);
                let st = load_balancer::state(
                    id,
                    &tokens,
                    balances.clone(),
                    scaling.clone(),
                    fee,
                    false,
                    curve.clone(),
                    (tokens[i], tokens[j]),
                )
                .unwrap();
                let PoolState::VaultPool(d) = st else {
                    panic!("не VaultPool")
                };
                let amount = U256::from_str_radix(q["amount"].as_str().unwrap(), 10).unwrap();
                let ours = balancer_math::amount_out(&d, i, j, amount);
                match q["answer"].get("ok") {
                    Some(a) => {
                        let deltas = dyn_list(&words(a), 0);
                        let out = I256::from_raw(deltas[j]);
                        assert_eq!(
                            ours.map(I256::from_raw),
                            Some(-out),
                            "пул {} {i}→{j}",
                            p["pool"]
                        );
                    }
                    None => assert_eq!(ours, None, "хранилище откатило — пул {}", p["pool"]),
                }
                checked += 1;
            }
        }
    }
    assert!(checked > 0, "в записи нет ответов queryBatchSwap");
}

#[test]
#[ignore = "запись 2026-11-20 пуста: событий создания пулов за окно 2 000 000 блоков нет (logs_total 0 у всех фабрик) — повтор sub_dex.py с большим --blocks (TASKS T456-03)"]
fn iziswap_factory_registry() {
    let z = json("izi.json");
    for (fee, d) in z["fee2pointDelta"].as_object().unwrap() {
        assert!(!words(&d["ok"])[0].is_zero(), "fee2pointDelta({fee}) = 0");
    }
    let mut checked = 0;
    for p in z["pools"].as_array().unwrap() {
        let (Some(xy), Some(yx)) = (p["pool_xy"].get("ok"), p["pool_yx"].get("ok")) else {
            continue;
        };
        let pool = addr(&p["pool"]);
        let fee = words(&p["fee()"]["ok"])[0].to::<u32>();
        assert_eq!(decode_q(Q::Izi(fee), &bytes(xy)).unwrap(), vec![pool]);
        assert_eq!(decode_q(Q::Izi(fee), &bytes(yx)).unwrap(), vec![pool]);
        checked += 1;
    }
    assert!(checked > 0, "в записи нет пулов iZiSwap");
}
