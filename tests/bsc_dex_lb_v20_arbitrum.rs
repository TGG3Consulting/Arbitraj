//! Файл: tests/bsc_dex_lb_v20_arbitrum.rs
//! Что проверяет: решение 445 — Liquidity Book v2.0 (lb_v20, LFJ V2 / Trader Joe V2 на Arbitrum One).
//! Без записи (идут всегда): селекторы и topic0 ABI v2.0 (pools/abi_lb20.rs — alloy) = keccak256 подписей исходника
//! lfj-gg/joe-v2@v2.0.0 и = константам скрипта записи tools/arbitrum_record/lb_v20.py; вид lb_v20 (LbBin, шаг
//! LB20 = 24 — решение 456, без данных).
//! На записи своего узла Arbitrum (tests/data/arbitrum_lb_v20_<дата>/, скрипт tools/arbitrum_record/lb_v20.py; запись
//! 2026-11-20 — без #[ignore], решение 461):
//! 1. Наша математика (load_lb20::decode → math/lb_swap.rs с ветками v20) на записанном состоянии, всех бинах и времени
//!    блока = LBRouter v2.0 getSwapOut (выход и комиссии) на каждой сумме обеих сторон; откат роутера — у нас неполный
//!    обмен или откат; запросы записи = наши кодировщики (calls / first_calls / calls_for).
//! 2. Проверка пула (verify_lb20 + verify_lb::decide) — все записанные пулы приняты.
//! 3. Чтение головы (lb20_head::decode) — то же состояние и резервы активного бина.
//! 4. События (events_lb20::bins_of) на записанных логах — бин события = id лога, активный — только у Swap.
//! 5. Слоты условия (6, 7, 10) — самопроверка exec/pool_cond_decode.rs differ: activeId слота = состояния.
//! 6. Квотер сверки (quoter_lb20::decode) по записанным ответам роутера.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{math::lb_swap, pools::{abi_lb20, load_lb20, lb20_head, lb_head, verify_lb20,
//! verify_lb, events_lb20, families, exec_kind, bins}, quote::quoter_lb20, exec::{pool_cond_decode, executor::kind}}.

use std::collections::BTreeMap;
use std::path::PathBuf;

use alloy::sol_types::{SolCall, SolEvent};
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use serde_json::Value;

use arb::core::types::{LbBinData, PoolKind, PoolState};
use arb::venues::bsc_dex::exec::executor::kind::Params;
use arb::venues::bsc_dex::exec::pool_cond_decode;
use arb::venues::bsc_dex::math::lb_swap;
use arb::venues::bsc_dex::pools::abi_lb20::{ILBPairV20 as P, ILBRouterV20 as R};
use arb::venues::bsc_dex::pools::bins::{BinMap, NONE_ABOVE, NONE_BELOW};
use arb::venues::bsc_dex::pools::exec_kind::{self, ExecKind};
use arb::venues::bsc_dex::pools::families::{self, Proto};
use arb::venues::bsc_dex::pools::verify::Candidate;
use arb::venues::bsc_dex::pools::{
    events_lb20, lb_head, lb20_head, load_lb20, load_lb20_bins, verify_lb, verify_lb20,
};
use arb::venues::bsc_dex::quote::quoter_lb20;

const NO_RECORD: &str = "нет записи: tools/arbitrum_record/lb_v20.py";

/// Папка записи: последняя tests/data/arbitrum_lb_v20_<дата>/. Ошибки: паника — записи нет.
fn dir() -> PathBuf {
    let root = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data"));
    let mut all: Vec<PathBuf> = std::fs::read_dir(&root)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("arbitrum_lb_v20_"))
        })
        .collect();
    all.sort();
    all.pop().expect(NO_RECORD)
}

fn json(name: &str) -> Value {
    let p = dir().join(name);
    serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap()
}

/// Записанные пулы (pool_<адрес>.json) по списку meta.json.
fn pools() -> Vec<Value> {
    json("meta.json")["pools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| json(&format!("pool_{}.json", p.as_str().unwrap().to_lowercase())))
        .collect()
}

/// Байты ответа eth_call записи (Ok) или текст ошибки узла (Err — откат).
fn answer(rec: &Value) -> Result<Bytes, String> {
    match rec["response"]["result"].as_str() {
        Some(h) => Ok(Bytes::from(alloy_primitives::hex::decode(h).unwrap())),
        None => Err(rec["response"]["error"].to_string()),
    }
}

/// Данные запроса eth_call записи.
fn input(rec: &Value) -> Bytes {
    let h = rec["request"]["params"][0]["input"].as_str().unwrap();
    Bytes::from(alloy_primitives::hex::decode(h).unwrap())
}

fn addr_of(b: &Bytes) -> Address {
    Address::from_slice(&b[12..32])
}

fn ts() -> u64 {
    json("meta.json")["timestamp"].as_u64().unwrap()
}

/// Состояние пула нашим разборщиком (load_lb20::decode) по записанным getReservesAndId и feeParameters.
fn state(p: &Value) -> LbBinData {
    let c = &p["calls"];
    let x = addr_of(&answer(&c["tokenX()"]).unwrap());
    let y = addr_of(&answer(&c["tokenY()"]).unwrap());
    let a = vec![
        answer(&c["getReservesAndId()"]).unwrap(),
        answer(&c["feeParameters()"]).unwrap(),
    ];
    load_lb20::decode(&a, x < y, ts()).unwrap()
}

/// Резервы бина из ответа getBin (два слова).
fn bin_of(rec: &Value) -> (u128, u128) {
    let b = answer(rec).unwrap();
    let r = P::getBinCall::abi_decode_returns(&b).unwrap();
    (r.reserveX.to(), r.reserveY.to())
}

/// Бины записи и края полосы: сторона, кончившаяся откатом findFirstNonEmptyBinId, — до конца пула (как загрузчик).
fn bins(p: &Value, d: &LbBinData) -> BinMap {
    let mut m = BTreeMap::new();
    let (ax, ay) = bin_of(&p["bins"]["active"]);
    if ax != 0 || ay != 0 {
        m.insert(d.active_id, (ax, ay));
    }
    let mut range = (d.active_id, d.active_id);
    for (side, down) in [("down", true), ("up", false)] {
        for s in p["bins"][side].as_array().unwrap() {
            if s.get("bin").is_none() {
                continue;
            }
            let id: u32 = U256::from_be_slice(&answer(&s["next"]).unwrap()).to();
            m.insert(id, bin_of(&s["bin"]));
            if down { range.0 = id } else { range.1 = id }
        }
        if p["bins"]["ends"][side] == "revert" {
            // Ревью 445: край стороны — откат с данными TreeMath__ErrorDepthSearch() или Panic(0x11) (load_lb20_bins edge).
            let last = p["bins"][side].as_array().unwrap().last().unwrap();
            let data = &last["next"]["response"]["error"]["data"];
            let data = alloy_primitives::hex::decode(data.as_str().unwrap()).unwrap();
            assert!(load_lb20_bins::edge(&data), "{side}: {}", last["next"]);
            if down {
                range.0 = NONE_ABOVE
            } else {
                range.1 = NONE_BELOW
            }
        }
    }
    BinMap {
        step: d.bin_step,
        reserves: m.into(),
        range,
    }
}

#[test]
fn selectors_and_topics_match_source_and_script() {
    let sel = |s: &str| keccak256(s)[..4].to_vec();
    assert_eq!(P::factoryCall::SELECTOR.to_vec(), sel("factory()"));
    assert_eq!(P::tokenXCall::SELECTOR.to_vec(), sel("tokenX()"));
    assert_eq!(P::tokenYCall::SELECTOR.to_vec(), sel("tokenY()"));
    assert_eq!(
        P::getReservesAndIdCall::SELECTOR.to_vec(),
        sel("getReservesAndId()")
    );
    assert_eq!(
        P::feeParametersCall::SELECTOR.to_vec(),
        sel("feeParameters()")
    );
    assert_eq!(
        P::findFirstNonEmptyBinIdCall::SELECTOR.to_vec(),
        sel("findFirstNonEmptyBinId(uint24,bool)")
    );
    assert_eq!(P::getBinCall::SELECTOR.to_vec(), sel("getBin(uint24)"));
    assert_eq!(
        R::getSwapOutCall::SELECTOR.to_vec(),
        sel("getSwapOut(address,uint256,bool)")
    );
    let ev = |s: &str| keccak256(s);
    assert_eq!(
        P::Swap::SIGNATURE_HASH,
        ev("Swap(address,address,uint256,bool,uint256,uint256,uint256,uint256)")
    );
    assert_eq!(
        P::DepositedToBin::SIGNATURE_HASH,
        ev("DepositedToBin(address,address,uint256,uint256,uint256)")
    );
    assert_eq!(
        P::WithdrawnFromBin::SIGNATURE_HASH,
        ev("WithdrawnFromBin(address,address,uint256,uint256,uint256)")
    );
    assert_eq!(
        P::CompositionFee::SIGNATURE_HASH,
        ev("CompositionFee(address,address,uint256,uint256,uint256)")
    );
    // Константы скрипта записи — строки вида `"подпись": "0x…",` — те же keccak256.
    let script = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tools/arbitrum_record/lb_v20.py"
    ))
    .unwrap();
    let mut n = 0;
    for line in script.lines().map(str::trim) {
        let Some((k, v)) = line.split_once("\": \"0x") else {
            continue;
        };
        let k = k.trim_start_matches('"');
        let v = v.trim_end_matches("\",");
        let want = if k.contains('(') && v.len() == 8 {
            alloy_primitives::hex::encode(sel(k))
        } else if v.len() == 64 {
            let sig = match k {
                "Swap" => "Swap(address,address,uint256,bool,uint256,uint256,uint256,uint256)",
                "DepositedToBin" => "DepositedToBin(address,address,uint256,uint256,uint256)",
                "WithdrawnFromBin" => "WithdrawnFromBin(address,address,uint256,uint256,uint256)",
                "CompositionFee" => "CompositionFee(address,address,uint256,uint256,uint256)",
                other => panic!("topic0 {other} не из ABI v2.0"),
            };
            alloy_primitives::hex::encode(ev(sig))
        } else {
            continue;
        };
        assert_eq!(v, want, "{k}");
        n += 1;
    }
    assert_eq!(n, 17, "13 селекторов и 4 topic0 скрипта");
}

#[test]
fn kind_and_step() {
    let f = families::make("traderjoe-v2-arbitrum@arbitrum", "lb_v20").expect("вид");
    assert_eq!((f.kind, f.proto), (PoolKind::LbBin, Proto::LbV20));
    assert!(f.graphable && !f.tradable && !f.fork);
    assert_eq!(f.executor_kind, Some(ExecKind::Lb20));
    assert_eq!(exec_kind::of("lb_v20"), Some(ExecKind::Lb20));
    assert_eq!(Params::Lb20.kind(), 24);
    assert!(Params::Lb20.data().is_empty());
    assert_eq!(lb20_head::per_pool(true), 3);
    assert_eq!(lb20_head::per_pool(false), lb_head::PER_POOL);
}

#[test]
fn formula_equals_router_get_swap_out() {
    let (mut equal, mut reverted) = (0, 0);
    for p in pools() {
        let d = state(&p);
        let b = bins(&p, &d);
        let pool: Address = p["pool"].as_str().unwrap().parse().unwrap();
        for q in p["quotes"].as_array().unwrap() {
            let for_y = q["swapForY"].as_bool().unwrap();
            let amount = U256::from_str_radix(q["amountIn"].as_str().unwrap(), 10).unwrap();
            // Запрос записи = наш кодировщик квотера (роутер, пара, сумма, сторона).
            let c = R::getSwapOutCall {
                pair: pool,
                amountIn: amount,
                swapForY: for_y,
            };
            assert_eq!(input(&q["call"]), Bytes::from(c.abi_encode()), "{q}");
            let Ok(amount) = u128::try_from(amount) else {
                continue;
            };
            let ours = lb_swap::swap_out(&d, &b, ts(), for_y, amount);
            let side_full = if for_y {
                b.down_to_end()
            } else {
                b.up_to_end()
            };
            match answer(&q["call"]) {
                Ok(out) => {
                    let r = R::getSwapOutCall::abi_decode_returns(&out).unwrap();
                    assert_eq!(quoter_lb20::decode(&out), Some(r.amountOut));
                    let Some(o) = ours else {
                        panic!("наш расчёт откатился, роутер — нет: {q}");
                    };
                    if !o.complete {
                        // Прочитана не вся сторона (предел --max-bins) — сравнивать не с чем.
                        assert!(!side_full, "вся сторона прочитана, а обмен неполный: {q}");
                        continue;
                    }
                    let got = (U256::from(o.out), U256::from(o.fee));
                    assert_eq!(got, (r.amountOut, r.feesIn), "{q}");
                    equal += 1;
                }
                Err(_) => {
                    let full = ours.is_some_and(|o| o.complete);
                    assert!(!full, "роутер откатился, у нас полный: {q}");
                    reverted += 1;
                }
            }
        }
    }
    assert!(
        equal > 0,
        "ни одного сравнения (равны {equal}, откатов {reverted})"
    );
}

#[test]
fn calls_match_encoders_and_head_decodes() {
    for p in pools() {
        let pool: Address = p["pool"].as_str().unwrap().parse().unwrap();
        let d = state(&p);
        let c = &p["calls"];
        let calls = load_lb20::calls(pool);
        assert_eq!(calls[0].1, input(&c["getReservesAndId()"]));
        assert_eq!(calls[1].1, input(&c["feeParameters()"]));
        let head = lb20_head::calls(pool, d.active_id);
        assert_eq!(head.len(), lb20_head::per_pool(true));
        assert_eq!(head[2].1, input(&p["bins"]["active"]));
        let a = vec![
            answer(&c["getReservesAndId()"]).unwrap(),
            answer(&c["feeParameters()"]).unwrap(),
            answer(&p["bins"]["active"]).unwrap(),
        ];
        let (h, bin) = lb_head::decode(&a, &d, ts()).unwrap();
        assert_eq!(h, d);
        assert_eq!(bin, bin_of(&p["bins"]["active"]));
        assert!(d.v20);
    }
}

#[test]
fn verify_takes_recorded_pools() {
    let fam = families::make("traderjoe-v2-arbitrum@arbitrum", "lb_v20").expect("вид");
    let meta = json("meta.json");
    let factory: Address = meta["factory"].as_str().unwrap().parse().unwrap();
    for p in pools() {
        let pool: Address = p["pool"].as_str().unwrap().parse().unwrap();
        let c = &p["calls"];
        let names = ["factory()", "tokenX()", "tokenY()", "feeParameters()"];
        for (i, (_, data)) in verify_lb20::first_calls(pool).iter().enumerate() {
            assert_eq!(*data, input(&c[names[i]]));
        }
        let a: Vec<Result<Bytes, String>> = names.iter().map(|n| answer(&c[*n])).collect();
        let cand = Candidate {
            family: fam,
            pool: p["pool"].as_str().unwrap().into(),
            coin: addr_of(&answer(&c["tokenX()"]).unwrap()),
            quote: addr_of(&answer(&c["tokenY()"]).unwrap()),
        };
        let f = verify_lb20::first_pass(&cand, &(pool, factory), &a).expect("первый проход");
        assert_eq!(
            verify_lb::second_call(&(pool, factory), &f).1,
            input(&c["getLBPairInformation"])
        );
        let info = answer(&c["getLBPairInformation"]);
        let found = verify_lb::decide(&cand, (pool, factory), f, &info).expect("реестр");
        assert_eq!(found.bin_step, Some(state(&p).bin_step));
    }
}

#[test]
fn events_give_touched_bins() {
    let mut n = 0;
    for p in json("meta.json")["pools"].as_array().unwrap() {
        let l = json(&format!("logs_{}.json", p.as_str().unwrap().to_lowercase()));
        for log in l["logs"]["response"]["result"].as_array().unwrap() {
            let topics: Vec<B256> = log["topics"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t.as_str().unwrap().parse().unwrap())
                .collect();
            let data = alloy_primitives::LogData::new(
                topics.clone(),
                Bytes::from(alloy_primitives::hex::decode(log["data"].as_str().unwrap()).unwrap()),
            )
            .unwrap();
            let (ids, active) = events_lb20::bins_of(topics[0], &data)
                .unwrap()
                .expect("событие про бины");
            let id: u32 = U256::from_be_bytes(topics[3].0).to();
            assert_eq!(ids, vec![id]);
            assert_eq!(active.is_some(), topics[0] == P::Swap::SIGNATURE_HASH);
            n += 1;
        }
    }
    assert!(n > 0, "в записи нет логов пулов");
}

#[test]
fn condition_slots_match_state() {
    for p in pools() {
        let d = state(&p);
        let w: Vec<U256> = ["6", "7", "10"]
            .iter()
            .map(|s| {
                let h = p["storage"][*s]["response"]["result"].as_str().unwrap();
                U256::from_be_slice(&alloy_primitives::hex::decode(h).unwrap())
            })
            .collect();
        let st = PoolState::LbBin(Box::new(d));
        assert_eq!(
            pool_cond_decode::differ(Proto::LbV20, &st, Some(w)),
            None,
            "{}",
            p["pool"]
        );
    }
}
