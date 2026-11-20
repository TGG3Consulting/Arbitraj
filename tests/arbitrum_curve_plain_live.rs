//! Файл: tests/arbitrum_curve_plain_live.rs
//! Что делает: решение 472 (T450-02) — вид curve_stableswap_plain (Curve StableSwap 2021) на записи своего узла
//! Arbitrum: (1) состояние собирается тем же кодом, что у бота (load_curve_plain::calls → ответы по calldata →
//! state_with: A_precise(), A_PRECISION = 100, строгая сходимость, pair_d у пула из двух монет); (2) наш выход
//! (math/stableswap_v1::exchange_out_t) = get_dy пула на каждой записанной сумме и паре (i, j) — побайтно; (3) рампы
//! на блоке записи нет или A_precise() совпадает с формулой _A() по initial/future A и времени блока; (4) у 2pool и
//! сверенных реализаций в коде есть селектор exchange(int128,int128,uint256,uint256) (шаг CURVE исполнителя).
//! Кто вызывает: cargo test. До записи — #[ignore].
//! От чего зависит: tests/data/arbitrum_curve_plain_<дата>/ (tools/arbitrum_record/curve_plain.py),
//! arb::venues::bsc_dex::{pools::{load_curve, load_curve_plain}, math::stableswap_v1}.

use std::path::PathBuf;

use alloy_primitives::{Address, Bytes, U256, hex};
use serde_json::Value;

use arb::core::types::PoolState;
use arb::gateway::errors::{ErrorCode, GatewayError};
use arb::venues::bsc_dex::math::stableswap_v1::{Template, exchange_out_t};
use arb::venues::bsc_dex::pools::load_curve::Raw;
use arb::venues::bsc_dex::pools::load_curve_plain;

/// Последняя по имени папка записи tests/data/arbitrum_curve_plain_*. Паника — записи нет.
fn data_dir() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
        .expect("tests/data")
        .filter_map(|d| d.ok().map(|d| d.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("arbitrum_curve_plain_"))
        })
        .collect();
    dirs.sort();
    dirs.pop().expect("нет записи: tools/arbitrum_record/curve_plain.py")
}

fn read(name: &str) -> Value {
    let text = std::fs::read_to_string(data_dir().join(name)).expect(name);
    serde_json::from_str(&text).expect("json")
}

fn bad(what: String) -> GatewayError {
    GatewayError::new(ErrorCode::BadResponse, None, what)
}

/// Ответ {"result": "0x…"} → байты; откат или нет ответа — Err(текст).
fn answer(v: Option<&Value>) -> Result<Bytes, String> {
    match v.and_then(|x| x.get("result")).and_then(|h| h.as_str()) {
        Some(h) => Ok(Bytes::from(hex::decode(h).expect("hex"))),
        None => Err(v.map_or_else(|| "нет ответа".to_string(), |x| x["error"].to_string())),
    }
}

fn int(v: Option<&Value>) -> U256 {
    let b = answer(v).expect("ответ");
    U256::from_be_slice(&b)
}

fn addr(s: &str) -> Address {
    s.parse().expect("адрес")
}

/// Raw пула по записанным ответам на calldata вызовов бота и decimals монет.
fn raw_of(p: &Value) -> Raw {
    let pool = addr(p["pool"].as_str().unwrap());
    let (calls, tail) = load_curve_plain::calls(pool);
    let got = calls
        .iter()
        .map(|(_, d)| answer(p["answers"].get(format!("0x{}", hex::encode(d)))))
        .collect();
    let decimals = p["coins"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            let d = int(p["decimals"].get(c.as_str().unwrap()));
            d.to::<u8>()
        })
        .collect();
    Raw { pool, got, tail, decimals }
}

/// Наш выход = get_dy пула на каждой записанной сумме и паре; шаблон Curve 2021 в состоянии.
#[test]
#[ignore = "нет записи: tools/arbitrum_record/curve_plain.py"]
fn math_equals_get_dy() {
    let mut checked = 0;
    for p in read("pools.json").as_array().unwrap() {
        let raw = raw_of(p);
        let coins: Vec<Address> = p["coins"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| addr(c.as_str().unwrap()))
            .collect();
        for q in p["get_dy"].as_array().unwrap() {
            let i = q["i"].as_u64().unwrap() as usize;
            let j = q["j"].as_u64().unwrap() as usize;
            let pair = if coins[i] < coins[j] {
                (coins[i], coins[j])
            } else {
                (coins[j], coins[i])
            };
            let st = load_curve_plain::state_with(&raw, pair, &bad).unwrap();
            let PoolState::StableSwap(d) = st else {
                panic!("не StableSwap")
            };
            assert_eq!(d.amp_precision, U256::from(100u8));
            assert!(d.strict);
            assert_eq!(d.pair_d, coins.len() == 2);
            let t = Template {
                amp_precision: d.amp_precision,
                max_rounds: d.max_rounds,
                strict: d.strict,
                pair_d: d.pair_d,
            };
            let dx = U256::from_str_radix(q["dx"].as_str().unwrap(), 10).unwrap();
            let ours = exchange_out_t(i, j, dx, &d.balances, &d.rates, d.amp, d.fee, &t);
            match q.get("result").and_then(|h| h.as_str()) {
                Some(h) => {
                    let pool = U256::from_be_slice(&hex::decode(h).unwrap());
                    assert_eq!(ours, Some(pool), "{} {i}->{j} dx {dx}", p["pool"]);
                    checked += 1;
                }
                None => assert_eq!(ours, None, "{} {i}->{j}: пул откатил", p["pool"]),
            }
        }
    }
    assert!(checked > 0, "ни одной сверки");
}

/// Рампа A: на блоке записи A_precise() = формула _A() (Plain2Basic.vy f389123 L230-L247).
#[test]
#[ignore = "нет записи: tools/arbitrum_record/curve_plain.py"]
fn a_precise_matches_ramp() {
    let now = U256::from(read("meta.json")["timestamp"].as_u64().unwrap());
    let sel = |s: &str| format!("0x{}", hex::encode(&alloy_primitives::keccak256(s)[..4]));
    for p in read("pools.json").as_array().unwrap() {
        let a = |s: &str| int(p["answers"].get(sel(s)));
        let (a0, a1) = (a("initial_A()"), a("future_A()"));
        let (t0, t1) = (a("initial_A_time()"), a("future_A_time()"));
        let want = if now < t1 {
            if a1 > a0 {
                a0 + (a1 - a0) * (now - t0) / (t1 - t0)
            } else {
                a0 - (a0 - a1) * (now - t0) / (t1 - t0)
            }
        } else {
            a1
        };
        assert_eq!(a("A_precise()"), want, "{}", p["pool"]);
        assert_eq!(a("A()"), want / U256::from(100u8), "{}", p["pool"]);
    }
}

/// Шаг CURVE исполнителя зовёт exchange(int128,int128,uint256,uint256): селектор есть в коде 2pool и реализаций.
#[test]
#[ignore = "нет записи: tools/arbitrum_record/curve_plain.py"]
fn exchange_selector_in_code() {
    let code = read("code.json");
    for (a, c) in code.as_object().unwrap() {
        assert!(c["len"].as_i64().unwrap() > 0, "{a}: кода нет");
        assert!(c["has_exchange4"].as_bool().unwrap(), "{a}: нет exchange(int128,int128,uint256,uint256)");
    }
}
