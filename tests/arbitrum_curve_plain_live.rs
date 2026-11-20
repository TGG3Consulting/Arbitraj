//! Файл: tests/arbitrum_curve_plain_live.rs
//! Что делает: решение 472 (T450-02, T472-01, T472-02) — виды curve_stableswap_plain и curve_stableswap_meta (Curve
//! StableSwap 2021) на записи своего узла Arbitrum: (1) состояние собирается тем же кодом, что у бота
//! (load_curve_plain::calls / load_curve_meta::calls → ответы по (адрес, calldata) → state_with: A_precise(),
//! A_PRECISION = 100, строгая сходимость, pair_d у plain-пула из двух монет, у метапула — курс LP = get_virtual_price
//! базового пула; метка ETH 0xEeee… — обёртка WETH; ema — ответ ma_exp_time()); (2) наш выход
//! (exchange_out_t и save_p модели stable_ema) = get_dy пула на каждой записанной сумме и паре (i, j) — побайтно;
//! (3) симуляция exchange на тех же суммах (eth_call с подменой баланса и разрешения): пул отдал выход — наш выход
//! тот же, пул откатил — наша котировка None; (4) A_precise() = формула _A() рампы; (5) селектор
//! exchange(int128,int128,uint256,uint256) в коде 2pool и реализаций (шаг CURVE исполнителя).
//! Кто вызывает: cargo test. До записи — #[ignore].
//! От чего зависит: tests/data/arbitrum_curve_plain_<дата>/ (tools/arbitrum_record/curve_plain.py),
//! arb::venues::bsc_dex::{pools::{load_curve, load_curve_plain, load_curve_meta}, math::{stableswap_v1, stable_ema}}.

use std::path::PathBuf;

use alloy_primitives::{Address, Bytes, U256, address, hex};
use serde_json::Value;

use arb::core::types::{PoolState, StableData};
use arb::gateway::errors::{ErrorCode, GatewayError};
use arb::venues::bsc_dex::math::stable_ema;
use arb::venues::bsc_dex::math::stableswap_v1::{Template, exchange_out_t};
use arb::venues::bsc_dex::pools::load_curve::Raw;
use arb::venues::bsc_dex::pools::{load_curve_meta, load_curve_plain};

/// WETH Arbitrum One — dex.native_wrapped площадки arb_dex (docs/notes/arbitrum/dex-settings-arbitrum.md §0).
const WETH: Address = address!("0x82aF49447D8a07e3bd95BD0d56f35241523fBab1");
const NATIVE: Address = address!("0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE");

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

fn addr(v: &Value) -> Address {
    v.as_str().expect("адрес").parse().expect("адрес")
}

/// Ключ ответа записи: «адрес|calldata» в нижнем регистре (curve_plain_pools.py key).
fn key(to: Address, data: &[u8]) -> String {
    format!("{to:#x}|0x{}", hex::encode(data))
}

fn coins(p: &Value) -> Vec<Address> {
    p["coins"].as_array().unwrap().iter().map(addr).collect()
}

/// Raw пула по записанным ответам на вызовы бота и decimals монет (метка ETH — 18).
fn raw_of(p: &Value) -> Raw {
    let pool = addr(&p["pool"]);
    let (calls, tail) = match p["kind"].as_str() {
        Some("meta") => load_curve_meta::calls(pool, addr(&p["base"])),
        _ => load_curve_plain::calls(pool),
    };
    let got = calls.iter().map(|(to, d)| answer(p["answers"].get(key(*to, d)))).collect();
    let decimals = coins(p)
        .iter()
        .map(|c| {
            if *c == NATIVE {
                return 18;
            }
            let b = answer(p["decimals"].get(format!("{c:#x}"))).expect("decimals");
            U256::from_be_slice(&b).to::<u8>()
        })
        .collect();
    Raw { pool, got, tail, decimals }
}

/// Состояние пары (i, j) пула и наш выход на dx: (exchange_out_t, пройдёт ли save_p модели stable_ema — у пула без
/// оракула всегда да). Паника — состояние не собралось.
fn ours(p: &Value, raw: &Raw, i: usize, j: usize, dx: U256) -> (Option<U256>, bool) {
    let cs: Vec<Address> = coins(p)
        .iter()
        .map(|c| if *c == NATIVE { WETH } else { *c })
        .collect();
    let pair = if cs[i] < cs[j] {
        (cs[i], cs[j])
    } else {
        (cs[j], cs[i])
    };
    let st = match p["kind"].as_str() {
        Some("meta") => load_curve_meta::state_with(raw, pair, &bad),
        _ => load_curve_plain::state_with(raw, pair, Some(WETH), &bad),
    };
    let PoolState::StableSwap(d) = st.unwrap_or_else(|e| panic!("{}: {e:?}", p["pool"])) else {
        panic!("не StableSwap")
    };
    let d: &StableData = &d;
    assert_eq!(d.amp_precision, U256::from(100u8));
    assert!(d.strict);
    assert_eq!(d.pair_d, p["kind"] != "meta" && cs.len() == 2);
    let t = Template {
        amp_precision: d.amp_precision,
        max_rounds: d.max_rounds,
        strict: d.strict,
        pair_d: d.pair_d,
    };
    let out = exchange_out_t(i, j, dx, &d.balances, &d.rates, d.amp, d.fee, &t);
    let ema = !d.ema || stable_ema::passes(i, j, dx, &d.balances, &d.rates, d.amp, &t).is_some();
    (out, ema)
}

/// Котировка бота (pool_quote.rs): выход, если и save_p не откатит.
fn quote(p: &Value, raw: &Raw, i: usize, j: usize, dx: U256) -> Option<U256> {
    let (out, ema) = ours(p, raw, i, j, dx);
    out.filter(|_| ema)
}

fn uint(h: &str) -> U256 {
    U256::from_be_slice(&hex::decode(h).unwrap())
}

/// Наш выход = get_dy пула на каждой записанной сумме и паре.
#[test]
#[ignore = "нет записи: tools/arbitrum_record/curve_plain.py"]
fn math_equals_get_dy() {
    let mut checked = 0;
    for p in read("pools.json").as_array().unwrap() {
        let raw = raw_of(p);
        for q in p["get_dy"].as_array().unwrap() {
            let (i, j) = (q["i"].as_u64().unwrap() as usize, q["j"].as_u64().unwrap() as usize);
            let dx = U256::from_str_radix(q["dx"].as_str().unwrap(), 10).unwrap();
            let Some(h) = q.get("result").and_then(|h| h.as_str()) else {
                continue;
            };
            // get_dy не проверяет save_p — сверяется выход без модели save_p.
            assert_eq!(
                ours(p, &raw, i, j, dx).0,
                Some(uint(h)),
                "{} {i}->{j} dx {dx}",
                p["pool"]
            );
            checked += 1;
        }
    }
    assert!(checked > 0, "ни одной сверки");
}

/// Симуляция exchange (T472-02): пул отдал выход — наш тот же; пул откатил — наша котировка None.
#[test]
#[ignore = "нет записи: tools/arbitrum_record/curve_plain.py"]
fn model_revert_equals_pool_revert() {
    let (mut ok, mut reverted) = (0, 0);
    for p in read("pools.json").as_array().unwrap() {
        let raw = raw_of(p);
        for s in p["exchange_sim"].as_array().into_iter().flatten() {
            // Подмена не подтверждена (слот не найден / проверка не прошла) — строка не доказательство.
            if s["ok"].is_null() || s["override_ok"] == Value::Bool(false) {
                continue;
            }
            let (i, j) = (s["i"].as_u64().unwrap() as usize, s["j"].as_u64().unwrap() as usize);
            let dx = U256::from_str_radix(s["dx"].as_str().unwrap(), 10).unwrap();
            let o = quote(p, &raw, i, j, dx);
            if s["ok"] == Value::Bool(true) {
                let want = uint(s["result"].as_str().unwrap());
                assert_eq!(o, Some(want), "{} {i}->{j} {dx}", p["pool"]);
                ok += 1;
            } else {
                assert_eq!(o, None, "{} {i}->{j} {dx}: пул откатил ({})", p["pool"], s["revert"]);
                reverted += 1;
            }
        }
    }
    println!("симуляций: выход {ok}, откат {reverted}");
    assert!(ok > 0, "ни одной симуляции с выходом");
}

/// Рампа A: на блоке записи A_precise() = формула _A() (Plain2Basic.vy f389123 L230-L247).
#[test]
#[ignore = "нет записи: tools/arbitrum_record/curve_plain.py"]
fn a_precise_matches_ramp() {
    let now = U256::from(read("meta.json")["timestamp"].as_u64().unwrap());
    for p in read("pools.json").as_array().unwrap() {
        let pool = addr(&p["pool"]);
        let a = |s: &str| {
            let sel = &alloy_primitives::keccak256(s)[..4];
            U256::from_be_slice(&answer(p["answers"].get(key(pool, sel))).expect(s))
        };
        let (a0, a1) = (a("initial_A()"), a("future_A()"));
        let (t0, t1) = (a("initial_A_time()"), a("future_A_time()"));
        let want = match now < t1 {
            true if a1 > a0 => a0 + (a1 - a0) * (now - t0) / (t1 - t0),
            true => a0 - (a0 - a1) * (now - t0) / (t1 - t0),
            false => a1,
        };
        assert_eq!(a("A_precise()"), want, "{pool}");
        assert_eq!(a("A()"), want / U256::from(100u8), "{pool}");
    }
}

/// Шаг CURVE исполнителя зовёт exchange(int128,int128,uint256,uint256): селектор есть в коде 2pool и реализаций.
#[test]
#[ignore = "нет записи: tools/arbitrum_record/curve_plain.py"]
fn exchange_selector_in_code() {
    let code = read("code.json");
    for (a, c) in code.as_object().unwrap() {
        assert!(c["len"].as_i64().unwrap() > 0, "{a}: кода нет");
        let has = c["has_exchange4"].as_bool().unwrap();
        assert!(has, "{a}: нет exchange(int128,int128,uint256,uint256)");
    }
}
