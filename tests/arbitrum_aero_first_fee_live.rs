//! Файл: tests/arbitrum_aero_first_fee_live.rs
//! Что проверяет: решение 470 (T470-01, T470-05, T470-06) на записи своего узла Arbitrum
//! (tests/data/arbitrum_aero_<дата>/record.json, tools/arbitrum_record/aero.py + aero_more.py; #[ignore] до записи):
//! - порт первой комиссии нового блока (pools/aero_first_fee.rs) на записанных параметрах крючка, конфиге пула,
//!   наблюдениях и observe([secondsAgo − δ, 0]) = fee() пула с blockOverrides {time: время + δ, number: блок + 1}
//!   (первый обмен нового блока; fee() — beforeSwapFee: 0 → tickSpacingToFee, предел 100000 — SwapHookLib);
//!   worst(…, 3) не ниже ни одного записанного fee() на + 0 / + 1 / + 3 с;
//! - Metaquoter quoteExactInputSingleV3 = виртуальный квотер бота (выход) на блоке и на + 1 / + 3 с;
//! - пулы Basic: getAmountOutWithTotalFee пула (шаг AERO_BASIC) = Metaquoter quoteExactInputSingleV2.
//!
//! Кто вызывает: cargo test (CI).
//! От чего зависит: arb::venues::bsc_dex::pools::aero_first_fee.

use alloy_primitives::U256;
use serde_json::Value;

use arb::venues::bsc_dex::pools::aero_first_fee::{self as ff, Config, Defaults, Head, Obs};

const NO_RECORD: &str = "нет записи: tools/arbitrum_record/aero.py";

fn record() -> Value {
    let base = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/");
    let mut found: Vec<String> = std::fs::read_dir(base)
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.starts_with("arbitrum_aero_"))
        .collect();
    found.sort();
    let dir = format!("{base}{}/record.json", found.last().expect(NO_RECORD));
    serde_json::from_str(&std::fs::read_to_string(dir).unwrap()).unwrap()
}

/// Слово i ответа eth_call. None — откат или короткий ответ.
fn w(v: &Value, i: usize) -> Option<U256> {
    let s = v.get("result")?.as_str()?.strip_prefix("0x")?;
    U256::from_str_radix(s.get(i * 64..(i + 1) * 64)?, 16).ok()
}

/// Знаковое слово, обрезанное до `bits` (int24 / int56).
fn signed(u: U256, bits: usize) -> i64 {
    let v = (u & ((U256::from(1u8) << bits) - U256::from(1u8))).to::<u64>() as i64;
    if v >> (bits - 1) & 1 == 1 {
        v - (1i64 << bits)
    } else {
        v
    }
}

fn small(v: &Value, i: usize) -> u64 {
    w(v, i).unwrap().to::<u64>()
}

/// Наблюдение (время, инициализировано). Нет записи — по умолчанию.
fn obs(v: &Value) -> Obs {
    match (w(v, 0), w(v, 3)) {
        (Some(t), Some(i)) => Obs {
            time: t.to::<u32>(),
            initialized: i == U256::from(1u8),
        },
        _ => Obs::default(),
    }
}

/// Ответ observe([S − δ, 0]): массив int56 — слова 4 и 5 (смещения двух массивов, длина 2, элементы).
fn cums(v: &Value) -> Option<(i64, i64)> {
    Some((signed(w(v, 3)?, 56), signed(w(v, 4)?, 56)))
}

#[test]
#[ignore = "нет записи: tools/arbitrum_record/aero.py"]
fn first_fee_port_equals_hook_fee_next_block() {
    let r = record();
    let time = r["timestamp"].as_u64().unwrap();
    let hp = &r["cl"]["hook"]["params"];
    let d = Defaults {
        seconds_ago: small(&hp["secondsAgo"], 0) as u32,
        scaling: w(&hp["defaultScalingFactor"], 0).unwrap(),
        fee_cap: w(&hp["defaultFeeCap"], 0).unwrap(),
    };
    let mut checked = 0;
    for p in r["cl"]["pools"].as_array().unwrap() {
        let c = &p["dynamicFeeConfig"];
        let cfg = Config {
            base_fee: small(c, 0) as u32,
            fee_cap: small(c, 1) as u32,
            scaling: small(c, 2),
            initial_enabled: small(c, 3) == 1,
            initial_fee: small(c, 4) as u32,
        };
        let ts_fee = small(&p["tickSpacingToFee"], 0) as u32;
        let s0 = &p["slot0"];
        let mut head = Head {
            time,
            tick: signed(w(s0, 1).unwrap(), 24) as i32,
            cardinality: small(s0, 3) as u16,
            oldest: obs(&p["observations_next"]),
            zero: obs(&p["observations_0"]),
            cums: vec![None; 4],
        };
        for (k, v) in p["observe"].as_object().into_iter().flatten() {
            head.cums[k.parse::<usize>().unwrap()] = cums(v);
        }
        let worst = ff::worst(&cfg, &d, ts_fee, &head, 3);
        let k = ff::scaling_of(&cfg, &d);
        for (dk, at) in p["fee_at"].as_object().unwrap() {
            let delta: u32 = dk.parse().unwrap();
            let Some(want) = w(&at["time_number"], 0) else {
                continue;
            };
            let (used, _) = ff::first(&cfg, &d, ts_fee, ff::dynamic_at(&head, &d, k, delta));
            // SwapHookLib: 0 — tickSpacingToFee, иначе не выше _SWAP_FEE_CEIL = 100000.
            let used = if used.is_zero() {
                U256::from(ts_fee)
            } else {
                used.min(U256::from(100_000u32))
            };
            assert_eq!(used, want, "пул {} δ {delta}", p["pool"]);
            assert!(
                U256::from(worst) >= want,
                "worst ниже fee() пула {} δ {delta}",
                p["pool"]
            );
            checked += 1;
        }
    }
    assert!(checked > 0, "в записи нет fee_at пулов CL");
}

#[test]
#[ignore = "нет записи: tools/arbitrum_record/aero.py"]
fn metaquoter_equals_virtual_and_total_fee() {
    let r = record();
    let mut checked = 0;
    for p in r["cl"]["pools"].as_array().unwrap() {
        for q in p["quotes_more"].as_array().into_iter().flatten() {
            for at in ["at_block", "at_plus_1", "at_plus_3"] {
                let virt = w(&q[at]["virtual"], 1);
                let (Some(m), Some(v)) = (w(&q[at]["metaquoter"], 0), virt) else {
                    continue;
                };
                assert_eq!(m, v, "пул {} {at} {q}", p["pool"]);
                checked += 1;
            }
        }
    }
    for side in ["volatile", "stable"] {
        for p in r[side]["pools"].as_array().unwrap() {
            for q in p["quotes"].as_array().unwrap() {
                let more = &q["more"];
                let total = w(&more["getAmountOutWithTotalFee"], 0);
                let (Some(t), Some(m)) = (total, w(&more["metaquoter"], 0)) else {
                    continue;
                };
                assert_eq!(t, m, "пул {} {q}", p["pool"]);
                checked += 1;
            }
        }
    }
    assert!(checked > 0, "в записи нет котировок Metaquoter");
}
