//! Файл: tests/bsc_dex_curve_family.rs
//! Что делает: семейство StableSwap — Ellipsis StableSwap (dex.kind = curve_stableswap, решение 122 F) без сети и без
//! моков: (1) таблица сопоставления → Proto::CurveStable, вид ядра StableSwap, в графе, без своего пути, шаг CURVE
//! исполнителя, нужна dex.stable_pools; (2) проверка пула по ответам coins(k) в ABI официального StableSwap.vy
//! (verify_curve::decide): пара среди монет — принят, нет — отказ; (3) сверка своей математики (exchange) с ответом
//! самого пула get_dy на ЖИВОЙ записи узла — ждёт записи tests/data/ellipsis/responses.json (запрос — tests/README.md),
//! пока записи нет — #[ignore].
//! Кто вызывает: cargo test (третий — cargo test --test bsc_dex_curve_family -- --ignored, когда запись есть).
//! От чего зависит: arb::venues::bsc_dex (families, exec_kind, verify_curve, abi_curve, math::stableswap_v1).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;

use arb::core::types::PoolKind;
use arb::venues::bsc_dex::math::stableswap_v1;
use arb::venues::bsc_dex::pools::abi_curve::IStableSwapV1;
use arb::venues::bsc_dex::pools::exec_kind::ExecKind;
use arb::venues::bsc_dex::pools::families::{self, Proto};
use arb::venues::bsc_dex::pools::verify::Candidate;
use arb::venues::bsc_dex::pools::verify_curve::decide;

/// Монеты BSC — адреса как у BingX (tests/data/indexers_2026-10-24/README.md и calls.json маршрутов): CAKE, USDT.
const CAKE: &str = "0x0E09FaBB73Bd3Ade0a17ECC321fD13a19e81cE82";
const USDT: &str = "0x55d398326f99059fF775485246999027B3197955";

fn a(s: &str) -> Address {
    s.parse().unwrap()
}

/// curve_stableswap: вид ядра StableSwap, в графе, без своего пути, шаг CURVE, нужна только dex.stable_pools.
#[test]
fn curve_family_flags() {
    let f = families::make("ellipsis", "curve_stableswap").unwrap();
    assert_eq!(f.proto, Proto::CurveStable);
    assert_eq!(f.kind, PoolKind::StableSwap);
    assert_eq!((f.graphable, f.tradable, f.fork), (true, false, false));
    assert_eq!(f.executor_kind, Some(ExecKind::Curve));
}

/// Ответ геттера coins(k) в ABI контракта (одно слово address); откат — Err.
fn coin_answer(x: Address) -> Result<Bytes, String> {
    Ok(IStableSwapV1::coinsCall::abi_encode_returns(&x).into())
}

/// Пара среди монет пула — пул принят (token0 < token1); пары нет — отказ с причиной.
#[test]
fn verify_decides_by_pool_coins() {
    let f = families::make("ellipsis", "curve_stableswap").unwrap();
    let pool = a("0x1111111111111111111111111111111111111111");
    let c = Candidate {
        family: f,
        pool: format!("{pool:#x}"),
        coin: a(CAKE),
        quote: a(USDT),
    };
    let revert = Err("execution reverted".to_string());
    let answers = vec![coin_answer(a(USDT)), coin_answer(a(CAKE)), revert.clone()];
    let found = decide(&c, pool, &answers).expect("пара в пуле");
    assert!(found.token0 < found.token1);
    let other = vec![coin_answer(a(USDT)), coin_answer(pool), revert];
    assert!(decide(&c, pool, &other).is_err());
}

/// Слово hex-ответа eth_call → байты.
fn hex(v: &Value) -> Vec<u8> {
    alloy_primitives::hex::decode(v.as_str().expect("строка").trim_start_matches("0x")).unwrap()
}

/// Своя математика (exchange) = ответ get_dy самого пула на одном блоке. Формат записи tests/data/ellipsis/
/// responses.json: {block, pools: [{pool, coins: [<result coins(k)>…], balances: [<result balances(k)>…], decimals:
/// [<число>…], A: <result A()>, fee: <result fee()>, quotes: [{i, j, dx: "<десятичное>", out: <result get_dy>}]}]}.
#[test]
#[ignore = "нет записи tests/data/ellipsis/responses.json — запрос записи в tests/README.md (решение 122 F)"]
fn ellipsis_math_matches_live_get_dy() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/ellipsis/responses.json"
    );
    let rec: Value = serde_json::from_str(&std::fs::read_to_string(path).expect("запись")).unwrap();
    for p in rec["pools"].as_array().expect("pools") {
        let word = |v: &Value| U256::from_be_slice(&hex(v));
        let balances: Vec<U256> = p["balances"].as_array().unwrap().iter().map(word).collect();
        let rates: Vec<U256> = p["decimals"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| stableswap_v1::rate_for_decimals(d.as_u64().unwrap() as u8).unwrap())
            .collect();
        let (amp, fee) = (word(&p["A"]), word(&p["fee"]));
        for q in p["quotes"].as_array().unwrap() {
            let (i, j) = (
                q["i"].as_u64().unwrap() as usize,
                q["j"].as_u64().unwrap() as usize,
            );
            let dx: U256 = q["dx"].as_str().unwrap().parse().unwrap();
            let got = stableswap_v1::exchange_out(i, j, dx, &balances, &rates, amp, fee);
            assert_eq!(
                got,
                Some(word(&q["out"])),
                "пул {} {i}→{j} dx {dx}",
                p["pool"]
            );
        }
    }
}
