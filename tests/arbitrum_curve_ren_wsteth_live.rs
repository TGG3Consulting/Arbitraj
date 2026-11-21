//! Файл: tests/arbitrum_curve_ren_wsteth_live.rs
//! Что делает: решение 481 (T472-10) — пулы Curve WBTC/renBTC 0x3E01…F0Fb (вид curve_stableswap_plain) и ETH/wstETH
//! 0x6eB2…7d80 (вид curve_stableswap_oracle) на записи своего узла Arbitrum: (1) состояние собирается тем же кодом, что
//! у бота (load_curve_plain::calls / load_curve_oracle::calls → ответы по (адрес, calldata) → state_with): у ren —
//! курсы 10^28 обеих монет (константа RATE_MULTIPLIER исходника L84 = правило 10^(36 − decimals) при 8 знаках), pair_d;
//! у wstETH — монета 0 — метка ETH (в паре WETH), курс 0 = 10^18, курс 1 = latestAnswer() оракула пула, D_P циклом;
//! (2) ren: наш выход (exchange_out_t) = get_dy пула побайтно (у исходника get_dy и exchange — оба с «− 1»);
//! wstETH: get_dy пула = формула get_dy исходника (БЕЗ «− 1», L416-L425) на нашем состоянии побайтно; (3) симуляция
//! exchange на тех же суммах (eth_call с подменой баланса / value ETH): пул отдал выход — наш выход (exchange_out_t,
//! «− 1» как в exchange L461) тот же; пул откатил — наша котировка None; (4) A_precise() = формула _A() рампы;
//! (5) селектор exchange(int128,int128,uint256,uint256) в коде обоих пулов, oracle() — в коде wstETH.
//! Кто вызывает: cargo test. До записи — #[ignore].
//! От чего зависит: tests/data/arbitrum_curve_ren_wsteth_<дата>/ (tools/arbitrum_record/curve_ren_wsteth.py),
//! arb::venues::bsc_dex::{pools::{load_curve, load_curve_plain, load_curve_oracle}, math::stableswap_v1}.
//! Исходники: docs/sources/curve_arbitrum/0x3E01dD8a5E1fb3481F0F589056b428Fc308AF0Fb.vy,
//! docs/sources/curve_arbitrum/0x6eB2dc694eB516B16Dc9FBc678C60052BbdD7d80.vy (номера строк — исходника Sourcify).

use std::path::PathBuf;

use alloy_primitives::{Address, Bytes, U256, address, hex};
use serde_json::Value;

use arb::core::types::{PoolState, StableData};
use arb::gateway::errors::{ErrorCode, GatewayError};
use arb::venues::bsc_dex::math::stableswap_v1::{
    Template, exchange_out_t, fee_denominator, get_y_t, precision, xp,
};
use arb::venues::bsc_dex::pools::load_curve::Raw;
use arb::venues::bsc_dex::pools::{load_curve_oracle, load_curve_plain};

/// WETH Arbitrum One — dex.native_wrapped площадки arb_dex (docs/notes/arbitrum/dex-settings-arbitrum.md §0).
const WETH: Address = address!("0x82aF49447D8a07e3bd95BD0d56f35241523fBab1");
const NATIVE: Address = address!("0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE");

/// Последняя по имени папка записи tests/data/arbitrum_curve_ren_wsteth_*. Паника — записи нет.
fn data_dir() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
        .expect("tests/data")
        .filter_map(|d| d.ok().map(|d| d.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("arbitrum_curve_ren_wsteth_"))
        })
        .collect();
    dirs.sort();
    dirs.pop()
        .expect("нет записи: tools/arbitrum_record/curve_ren_wsteth.py")
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

fn is_oracle(p: &Value) -> bool {
    p["kind"] == "oracle"
}

/// Raw пула по записанным ответам на вызовы бота и decimals монет (метка ETH — 18).
fn raw_of(p: &Value) -> Raw {
    let pool = addr(&p["pool"]);
    let (calls, tail) = if is_oracle(p) {
        load_curve_oracle::calls(pool, addr(&p["oracle"]))
    } else {
        load_curve_plain::calls(pool)
    };
    let got = calls
        .iter()
        .map(|(to, d)| answer(p["answers"].get(key(*to, d))))
        .collect();
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
    Raw {
        pool,
        got,
        tail,
        decimals,
    }
}

/// Состояние пары (i, j) пула (код бота) и шаблон. Паника — состояние не собралось.
fn state(p: &Value, raw: &Raw, i: usize, j: usize) -> (StableData, Template) {
    let cs: Vec<Address> = coins(p)
        .iter()
        .map(|c| if *c == NATIVE { WETH } else { *c })
        .collect();
    let pair = if cs[i] < cs[j] {
        (cs[i], cs[j])
    } else {
        (cs[j], cs[i])
    };
    let st = if is_oracle(p) {
        load_curve_oracle::state_with(raw, pair, Some(WETH), &bad)
    } else {
        load_curve_plain::state_with(raw, pair, Some(WETH), &bad)
    };
    let PoolState::StableSwap(d) = st.unwrap_or_else(|e| panic!("{}: {e:?}", p["pool"])) else {
        panic!("не StableSwap")
    };
    assert_eq!(d.amp_precision, U256::from(100u8));
    assert!(d.strict);
    assert!(!d.ema, "{}: ma_exp_time() у этих пулов нет", p["pool"]);
    let t = Template {
        amp_precision: d.amp_precision,
        max_rounds: d.max_rounds,
        strict: d.strict,
        pair_d: d.pair_d,
    };
    (*d, t)
}

fn uint(h: &str) -> U256 {
    U256::from_be_slice(&hex::decode(h).unwrap())
}

/// get_dy исходника wstETH (L416-L425): `dy = xp[j] − y` без «− 1», комиссия с dy, `(dy − fee)·1e18/rates[j]`.
fn get_dy_wsteth(i: usize, j: usize, dx: U256, d: &StableData, t: &Template) -> Option<U256> {
    let xp = xp(&d.balances, &d.rates)?;
    let p = precision();
    let x = xp[i].checked_add(dx.checked_mul(d.rates[i])? / p)?;
    let y = get_y_t(i, j, x, &xp, d.amp, t)?;
    let dy = xp[j].checked_sub(y)?;
    let fee = d.fee.checked_mul(dy)? / fee_denominator();
    Some(dy.checked_sub(fee)?.checked_mul(p)? / d.rates[j])
}

/// Курсы и шаблон по исходникам: ren — RATE_MULTIPLIER = 10^28 обеих монет, pair_d; wstETH — [10^18, latestAnswer],
/// метка ETH — монета 0, D_P циклом.
#[test]
#[ignore = "нет записи: tools/arbitrum_record/curve_ren_wsteth.py"]
fn rates_match_sources() {
    for p in read("pools.json").as_array().unwrap() {
        let raw = raw_of(p);
        let (d, _) = state(p, &raw, 0, 1);
        if is_oracle(p) {
            let o = addr(&p["oracle"]);
            let sel = &alloy_primitives::keccak256("latestAnswer()")[..4];
            let want = U256::from_be_slice(&answer(p["answers"].get(key(o, sel))).expect("курс"));
            assert_eq!(d.native, Some(0));
            assert!(!d.pair_d);
            assert_eq!(d.rates, vec![U256::from(10u8).pow(U256::from(18u8)), want]);
        } else {
            let r = U256::from(10u8).pow(U256::from(28u8));
            assert_eq!(d.native, None);
            assert!(d.pair_d);
            assert_eq!(d.rates, vec![r, r], "{}", p["pool"]);
        }
    }
}

/// get_dy пула: у ren = наш выход exchange; у wstETH = формула get_dy исходника (без «− 1») на нашем состоянии.
#[test]
#[ignore = "нет записи: tools/arbitrum_record/curve_ren_wsteth.py"]
fn get_dy_matches() {
    let mut checked = 0;
    for p in read("pools.json").as_array().unwrap() {
        let raw = raw_of(p);
        for q in p["get_dy"].as_array().unwrap() {
            let (i, j) = (
                q["i"].as_u64().unwrap() as usize,
                q["j"].as_u64().unwrap() as usize,
            );
            let dx = U256::from_str_radix(q["dx"].as_str().unwrap(), 10).unwrap();
            let Some(h) = q.get("result").and_then(|h| h.as_str()) else {
                continue;
            };
            let (d, t) = state(p, &raw, i, j);
            let ours = if is_oracle(p) {
                get_dy_wsteth(i, j, dx, &d, &t)
            } else {
                exchange_out_t(i, j, dx, &d.balances, &d.rates, d.amp, d.fee, &t)
            };
            assert_eq!(ours, Some(uint(h)), "{} {i}->{j} dx {dx}", p["pool"]);
            checked += 1;
        }
    }
    assert!(checked > 0, "ни одной сверки");
}

/// Симуляция exchange: пул отдал выход — наш выход exchange_out_t тот же; пул откатил — наш выход None.
#[test]
#[ignore = "нет записи: tools/arbitrum_record/curve_ren_wsteth.py"]
fn exchange_sim_matches() {
    let (mut ok, mut reverted) = (0, 0);
    for p in read("pools.json").as_array().unwrap() {
        let raw = raw_of(p);
        for s in p["exchange_sim"].as_array().into_iter().flatten() {
            // Подмена не подтверждена (слот не найден / проверка не прошла) — строка не доказательство.
            if s["ok"].is_null() || s["override_ok"] == Value::Bool(false) {
                continue;
            }
            let (i, j) = (
                s["i"].as_u64().unwrap() as usize,
                s["j"].as_u64().unwrap() as usize,
            );
            let dx = U256::from_str_radix(s["dx"].as_str().unwrap(), 10).unwrap();
            let (d, t) = state(p, &raw, i, j);
            let o = exchange_out_t(i, j, dx, &d.balances, &d.rates, d.amp, d.fee, &t);
            if s["ok"] == Value::Bool(true) {
                let want = uint(s["result"].as_str().unwrap());
                assert_eq!(o, Some(want), "{} {i}->{j} {dx}", p["pool"]);
                ok += 1;
            } else {
                assert_eq!(
                    o, None,
                    "{} {i}->{j} {dx}: пул откатил ({})",
                    p["pool"], s["revert"]
                );
                reverted += 1;
            }
        }
    }
    println!("симуляций: выход {ok}, откат {reverted}");
    assert!(ok > 0, "ни одной симуляции с выходом");
}

/// Рампа A: на блоке записи A_precise() = формула _A() (ren L237-L254, wstETH L161-L178).
#[test]
#[ignore = "нет записи: tools/arbitrum_record/curve_ren_wsteth.py"]
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

/// Шаг CURVE / CURVE_ETH исполнителя зовёт exchange(int128,int128,uint256,uint256): селектор есть в коде обоих пулов;
/// у wstETH в коде есть oracle() (адрес оракула — его ответ).
#[test]
#[ignore = "нет записи: tools/arbitrum_record/curve_ren_wsteth.py"]
fn selectors_in_code() {
    let code = read("code.json");
    let pools = read("meta.json")["pools"].clone();
    for a in pools.as_array().unwrap() {
        let c = &code[a.as_str().unwrap()];
        assert!(c["len"].as_i64().unwrap() > 0, "{a}: кода нет");
        assert!(
            c["has_exchange4"].as_bool().unwrap(),
            "{a}: нет exchange из 4 аргументов"
        );
    }
    let w = &code["0x6eB2dc694eB516B16Dc9FBc678C60052BbdD7d80"];
    assert!(w["has_oracle"].as_bool().unwrap(), "wstETH: нет oracle()");
}
