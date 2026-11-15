//! Файл: tests/bsc_dex_lb_v21_live.rs
//! Что проверяет: решение 290 — Liquidity Book V2.1 (lb_v21, LFJ / Trader Joe на BSC) на РЕАЛЬНЫХ ответах узла BSC,
//! записанных на одном блоке 133716961 (tests/data/lb_v21_2026-11-15): пять пулов фабрики 0x8e42…fd87d3efd4e (шаги
//! бина 1, 25, 100, 20, 1; обе реализации пула; X > Y по адресу у USDC/USDT и BUSD/USDT), все непустые бины каждого.
//! 1. Наша математика (load_lb::decode → math/lb_swap.rs) на записанном состоянии и времени блока даёт ТОТ ЖЕ ответ, что
//!    getSwapOut пула (остаток входа, выход, комиссия) — 50 из 50 (5 пулов × 5 сумм × 2 стороны, в т. ч. вход больше
//!    ликвидности: остаток > 0, complete = false).
//! 2. Шаг исполнителя на цепи (exec_state.json, тот же блок): наш кодировщик шага (Params::Lb → kind::encode → execute)
//!    даёт побайтно записанную calldata; выход шага 18 исполнителя v2 (eth_call с подменой кода) = getSwapOut = наша
//!    формула в 10 из 10 (9 одиночных шагов обеих сторон и маршрут из двух шагов); вход больше ликвидности — откат
//!    LBPair__OutOfLiquidity (0xd36bfd88) и у нас complete = false; шаг LB в пару PancakeSwap V2 — откат.
//! 3. Проверка пула (verify_lb): все пять пулов приняты по записанным ответам; настоящий пул другой DEX (PancakeSwap V2:
//!    getFactory() откатывается) и монета не из пары пула — отказ; без подмены ответов.
//! 4. Квотер сверки (quoter_lb::decode) по записанным ответам getSwapOut; вид LB = 18, только v2; topic0 событий.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{math::lb_swap, pools::{load_lb, verify_lb, bins, families, exec_kind,
//! exec_callback, abi_lb}, quote::quoter_lb, exec::executor::{kind, abi}}.

use std::collections::BTreeMap;

use alloy::sol_types::{SolCall, SolValue};
use alloy_primitives::{Address, Bytes, U256, keccak256};
use serde_json::Value;

use arb::core::types::{LbBinData, PoolKind};
use arb::venues::bsc_dex::exec::executor::abi::ArbExecutor;
use arb::venues::bsc_dex::exec::executor::kind::{self, ALL_BPS, Params, StepSpec};
use arb::venues::bsc_dex::math::lb_swap;
use arb::venues::bsc_dex::pools::abi_lb::{self, ILBPair};
use arb::venues::bsc_dex::pools::bins::BinMap;
use arb::venues::bsc_dex::pools::exec_kind::{self, ExecKind};
use arb::venues::bsc_dex::pools::verify::Candidate;
use arb::venues::bsc_dex::pools::{exec_callback, families, load_lb, verify_lb};
use arb::venues::bsc_dex::quote::quoter_lb;

const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/lb_v21_2026-11-15/");

fn json(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(format!("{DIR}{name}")).unwrap()).unwrap()
}

fn addr(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}

fn dec(v: &Value) -> U256 {
    U256::from_str_radix(v.as_str().unwrap(), 10).unwrap()
}

fn u128s(v: &Value) -> u128 {
    v.as_str().unwrap().parse().unwrap()
}

/// Слова ответа eth_call (каждое — записанное десятичное число) → байты ответа, как их отдал узел.
fn words(v: &[&Value]) -> Bytes {
    let mut b = Vec::new();
    for w in v {
        b.extend_from_slice(&dec(w).to_be_bytes::<32>());
    }
    Bytes::from(b)
}

fn pool_of<'a>(probe: &'a Value, a: &str) -> &'a Value {
    probe["pools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["pool"] == a)
        .unwrap()
}

/// Состояние пула тем же разборщиком, что у чтения на узле (load_lb::decode), по записанным ответам getActiveId,
/// getStaticFeeParameters, getVariableFeeParameters. X — token0, если адрес X меньше адреса Y.
fn state(probe: &Value, p: &Value) -> LbBinData {
    let s: Vec<&Value> = p["static"].as_array().unwrap().iter().collect();
    let v: Vec<&Value> = p["variable"].as_array().unwrap().iter().collect();
    let answers = vec![words(&[&p["activeId"]]), words(&s), words(&v)];
    let x_is_token0 = addr(&p["tokenX"]) < addr(&p["tokenY"]);
    let step: u32 = p["binStep"].as_str().unwrap().parse().unwrap();
    load_lb::decode(
        &answers,
        x_is_token0,
        step,
        probe["timestamp"].as_u64().unwrap(),
    )
    .unwrap()
}

/// Бины пула (X, Y) — все непустые (обход записан до конца в обе стороны: getNextNonEmptyBin дошёл до края).
fn bins(p: &Value) -> BinMap {
    let mut m = BTreeMap::new();
    for (k, v) in p["bins"].as_object().unwrap() {
        m.insert(k.parse::<u32>().unwrap(), (u128s(&v[0]), u128s(&v[1])));
    }
    BinMap {
        step: p["binStep"].as_str().unwrap().parse().unwrap(),
        reserves: m.into(),
        range: (0, 0xff_ffff),
    }
}

#[test]
fn formula_equals_get_swap_out() {
    let probe = json("probe.json");
    let ts = probe["timestamp"].as_u64().unwrap();
    let mut n = 0;
    for p in probe["pools"].as_array().unwrap() {
        let d = state(&probe, p);
        let b = bins(p);
        for q in p["quotes"].as_array().unwrap() {
            let for_y = q["swapForY"].as_bool().unwrap();
            let r = lb_swap::swap_out(&d, &b, ts, for_y, u128s(&q["amountIn"])).expect("расчёт");
            let o = &q["out"];
            assert_eq!(
                (r.in_left, r.out, r.fee),
                (u128s(&o[0]), u128s(&o[1]), u128s(&o[2])),
                "{} {q}",
                p["pool"]
            );
            assert_eq!(r.complete, r.in_left == 0);
            n += 1;
        }
    }
    assert_eq!(n, 50);
}

#[test]
fn exec_state_step_calldata_and_received() {
    let probe = json("probe.json");
    let st = json("exec_state.json");
    assert_eq!(st["block"], probe["block"]);
    let ts = probe["timestamp"].as_u64().unwrap();
    let (mut ok, mut out_of_liquidity, mut foreign) = (0, 0, 0);
    for c in st["exec"].as_array().unwrap() {
        let hops: Vec<&Value> = c["steps"].as_array().unwrap().iter().collect();
        // Наш кодировщик шага (Params::Lb — как executor/entry.rs) → calldata execute побайтно = записанной.
        let steps: Vec<_> = hops
            .iter()
            .map(|h| {
                kind::encode(&StepSpec {
                    params: Params::Lb,
                    pool: addr(&h[0]),
                    token_in: addr(&h[1]),
                    token_out: addr(&h[2]),
                    share_bps: ALL_BPS,
                })
                .unwrap()
            })
            .collect();
        let call = ArbExecutor::executeCall {
            tokenIn: addr(&c["token_in"]),
            amountIn: U256::ZERO,
            tokenOut: addr(&c["token_out"]),
            minOut: U256::ZERO,
            deadline: U256::from(4_102_444_800u64),
            steps,
        };
        let want = alloy_primitives::hex::decode(c["calldata"].as_str().unwrap()).unwrap();
        assert_eq!(call.abi_encode(), want, "{}", c["steps"]);
        let lb_pools: Vec<&Value> = probe["pools"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|p| hops.iter().any(|h| h[0] == p["pool"]))
            .collect();
        if lb_pools.len() != hops.len() {
            // Шаг LB в пару PancakeSwap V2 (не LB): getTokenX откатывается — исполнитель откатил весь вызов.
            assert!(c["received"].is_null() && c["err"].as_str().unwrap().contains("reverted"));
            foreign += 1;
            continue;
        }
        // Наша формула по цепочке шагов на состоянии того же блока.
        let mut amount = u128s(&c["amount"]);
        let mut complete = true;
        for h in &hops {
            let p = pool_of(&probe, h[0].as_str().unwrap());
            let d = state(&probe, p);
            let for_y = addr(&h[1]) == addr(&p["tokenX"]);
            let r = lb_swap::swap_out(&d, &bins(p), ts, for_y, amount).unwrap();
            complete &= r.complete;
            amount = r.out;
        }
        if c["err"].is_null() {
            assert!(complete, "{c}");
            assert_eq!(U256::from(amount), dec(&c["received"]), "{c}");
            let last = c["getSwapOut"].as_array().unwrap().last().unwrap();
            assert_eq!(dec(&c["received"]), dec(&last[2]));
            ok += 1;
        } else {
            // Вход больше ликвидности пула: исполнитель откатил с LBPair__OutOfLiquidity, у нас — неполный обмен.
            let sel =
                alloy_primitives::hex::encode_prefixed(&keccak256("LBPair__OutOfLiquidity()")[..4]);
            assert!(c["err"].as_str().unwrap().ends_with(&sel), "{c}");
            assert!(!complete, "{c}");
            out_of_liquidity += 1;
        }
    }
    assert_eq!((ok, out_of_liquidity, foreign), (10, 2, 1));
}

/// Ответы первого прохода проверки (getFactory, getTokenX, getTokenY, getBinStep) — записанные значения пула.
fn first(p: &Value) -> Vec<Result<Bytes, String>> {
    vec![
        Ok(Bytes::from(addr(&p["getFactory"]).abi_encode())),
        Ok(Bytes::from(addr(&p["tokenX"]).abi_encode())),
        Ok(Bytes::from(addr(&p["tokenY"]).abi_encode())),
        Ok(words(&[&p["binStep"]])),
    ]
}

#[test]
fn verify_takes_factory_pools_and_rejects_others() {
    let probe = json("probe.json");
    let fam = families::make("traderjoe-v2-1-bsc", "lb_v21").expect("вид");
    let factory = addr(&probe["factory"]);
    for p in probe["pools"].as_array().unwrap() {
        let pool = addr(&p["pool"]);
        let cand = Candidate {
            family: fam,
            pool: p["pool"].as_str().unwrap().into(),
            coin: addr(&p["tokenX"]),
            quote: addr(&p["tokenY"]),
        };
        let f = verify_lb::first_pass(&cand, &(pool, factory), &first(p)).expect("первый проход");
        assert_eq!(verify_lb::first_calls(pool).len(), verify_lb::FIRST);
        let info: Vec<&Value> = p["factoryInfo"].as_array().unwrap().iter().collect();
        let found =
            verify_lb::decide(&cand, (pool, factory), f, &Ok(words(&info))).expect("реестр");
        assert_eq!(
            found.bin_step,
            Some(p["binStep"].as_str().unwrap().parse().unwrap())
        );
        assert!(found.token0 < found.token1);
    }
    // Настоящий пул другой DEX (PancakeSwap V2): getFactory() откатывается — отказ первого прохода.
    let fr = &probe["foreign"];
    let rev = |v: &Value| Err::<Bytes, String>(v["err"].as_str().unwrap().to_string());
    let a = vec![
        rev(&fr["getFactory"]),
        rev(&fr["getTokenX"]),
        rev(&fr["getTokenX"]),
        rev(&fr["getBinStep"]),
    ];
    let cand = Candidate {
        family: fam,
        pool: fr["pool"].as_str().unwrap().into(),
        coin: "0x55d398326f99059ff775485246999027b3197955"
            .parse()
            .unwrap(),
        quote: "0xbb4cdb9cbd36b01bd1cbaebf2de08d9173bc095c"
            .parse()
            .unwrap(),
    };
    let r = verify_lb::first_pass(&cand, &(addr(&fr["pool"]), factory), &a);
    assert!(r.unwrap_err().contains("getFactory"));
    // Пул BTC.b / WBNB с монетой USDT, которой в паре пула нет — отказ (ответы пула записанные).
    let p = pool_of(&probe, "0x717d06b4d8bc6b71d5a2c45d8cc417930aec073d");
    let cand = Candidate {
        family: fam,
        pool: p["pool"].as_str().unwrap().into(),
        coin: "0x55d398326f99059ff775485246999027b3197955"
            .parse()
            .unwrap(),
        quote: addr(&p["tokenY"]),
    };
    let r = verify_lb::first_pass(&cand, &(addr(&p["pool"]), factory), &first(p));
    assert!(r.unwrap_err().contains("токены пула"));
}

#[test]
fn quoter_decodes_recorded_get_swap_out() {
    let probe = json("probe.json");
    let mut n = 0;
    for p in probe["pools"].as_array().unwrap() {
        for q in p["quotes"].as_array().unwrap() {
            let o: Vec<&Value> = q["out"].as_array().unwrap().iter().collect();
            let got = quoter_lb::decode(&words(&o));
            let want = (o[0] == "0").then(|| dec(o[1]));
            assert_eq!(got, want, "{q}");
            n += 1;
        }
    }
    assert_eq!(n, 50);
    assert_eq!(
        ILBPair::getSwapOutCall::SELECTOR,
        keccak256("getSwapOut(uint128,bool)")[..4]
    );
}

#[test]
fn kind_step_and_events() {
    let f = families::make("traderjoe-v2-1-bsc", "lb_v21").expect("вид");
    assert_eq!(f.kind, PoolKind::LbBin);
    assert!(f.graphable && !f.tradable && !f.fork);
    assert_eq!(f.executor_kind, Some(ExecKind::Lb));
    assert_eq!(exec_kind::of("lb_v21"), Some(ExecKind::Lb));
    assert!(exec_callback::v2_only(ExecKind::Lb));
    assert_eq!(Params::Lb.kind(), 18);
    assert!(Params::Lb.data().is_empty());
    let want: Vec<_> = [
        "DepositedToBins(address,address,uint256[],bytes32[])",
        "WithdrawnFromBins(address,address,uint256[],bytes32[])",
        "CompositionFees(address,uint24,bytes32,bytes32)",
        "CollectedProtocolFees(address,bytes32)",
        "Swap(address,address,uint24,bytes32,bytes32,uint24,bytes32,bytes32)",
        "StaticFeeParametersSet(address,uint16,uint16,uint16,uint16,uint24,uint16,uint24)",
        "FlashLoan(address,address,uint24,bytes32,bytes32,bytes32)",
        "ForcedDecay(address,uint24,uint24)",
    ]
    .iter()
    .map(keccak256)
    .collect();
    assert_eq!(abi_lb::topics(), want);
}
