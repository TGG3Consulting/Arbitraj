//! Файл: tests/arb_curve_crypto_live.rs
//! Что проверяет: решения 442, 453 — crypto-пулы Curve Arbitrum One (Twocrypto-NG v2.1.0 и v2.0.0, Tricrypto-NG,
//! crypto v1 фабрики, tricrypto 2021) на РЕАЛЬНЫХ ответах своего узла Arbitrum, записанных на одном блоке скриптом
//! tools/arbitrum_record/curve_crypto.py (tests/data/arbitrum_curve_crypto_<дата>/twocrypto.json, tricrypto.json,
//! crypto_v1.json, tricrypto_v1.json):
//! 1. Ответы берутся по calldata вызовов бота (pools/load_crypto.rs и load_crypto_v1.rs calls()) — порядок и данные
//!    вызовов те же, что у бота (ревью 442, п. 8); разбор — decode бота.
//! 2. Наша математика (math/crypto_swap.rs exchange_out: формула версии пула, рампа — newton_D) на записанном состоянии
//!    даёт ТОТ ЖЕ ответ, что get_dy пула, на каждой сумме лесенки в обе стороны; откат get_dy — у нас None. Пулы вне
//!    списков математики (MATH() не из записанных `maths`) и версии не из списка не сверяются (как verify_crypto.rs), но
//!    сверенным должен быть хотя бы один пул каждой математики (ревью 442, п. 1).
//! 3. Код (eth_getCode) математики и реализаций Twocrypto-NG — ровно код, собранный из исходника (Vyper 0.3.10,
//!    twocrypto-ng d612fde — v2.0.0, 99b5192 — v2.1.0; хеши ниже), код пулов начинается кодом своей реализации;
//!    у пулов фабрики crypto v1 — прокси EIP-1167 на шаблон из записи (verify_crypto_v1.rs proxy_target).
//!
//! Пока записи нет, тесты помечены #[ignore] — снять после записи со своего узла (данных узла Arbitrum у разработчика
//! нет; тесты — только на записанных ответах, CLAUDE.md).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{math::crypto_swap, pools::{load_crypto, load_crypto_v1, crypto_variant,
//! verify_crypto_v1}, quote::quoter_crypto}. Моков нет.

use std::path::PathBuf;
use std::str::FromStr;

use alloy_primitives::{Address, Bytes, U256, keccak256};
use serde_json::Value;

use arb::core::types::CryptoData;
use arb::venues::bsc_dex::math::crypto_swap;
use arb::venues::bsc_dex::pools::verify_crypto_v1::proxy_target;
use arb::venues::bsc_dex::pools::{crypto_variant, load_crypto, load_crypto_v1};
use arb::venues::bsc_dex::quote::quoter_crypto;

/// Хеши кода Twocrypto-NG, собранного из исходника Vyper 0.3.10 (CREATE2-адреса сборки совпали с deployments.yaml):
/// (имя, длина, keccak256). Математика — код целиком; реализация (blueprint) — 0xFE7100 + код развёртывания; пул —
/// начало кода (runtime реализации, за ним — immutables).
const MATH_V210: (usize, &str) =
    (10137, "70d105ee0e9b857244d87521bbcef34cfb4cbd600592af61bbc7c76180648d61");
const MATH_V200: (usize, &str) =
    (9964, "bc931b24ccca3aae2cd24bf83022386ae21d4c96af2f7c7f57a91c743b8352e3");
const AMM_V210: (usize, &str) =
    (18837, "cb85feb205dc33b1704e8b492cb1d22c4b6728e23558123618b30216a15dea62");
const AMM_V200: (usize, &str) =
    (20742, "29fefecf08c8794a6a8af8d5382373a2ac7364f81e25f4950894dbaed4eef22d");
const POOL_V210: (usize, &str) =
    (17875, "e40fd9046a6204d8e7985312d45a721dae1ff759dd9d91358f06024c5ec4bc37");
const POOL_V200: (usize, &str) =
    (19738, "fe2e067b2dffd2dc7f5af5fe2dc58ab997acbceee805e562e569237ecd323a74");

/// Папка записи: последняя по имени tests/data/arbitrum_curve_crypto_<дата>. Паника — записи нет.
fn dir() -> PathBuf {
    let base = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data"));
    let mut found: Vec<PathBuf> = std::fs::read_dir(&base)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("arbitrum_curve_crypto_"))
        })
        .collect();
    found.sort();
    found.pop().expect("нет записи: tools/arbitrum_record/curve_crypto.py")
}

/// Файл записи. Паника — нет файла.
fn doc(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(dir().join(name)).unwrap()).unwrap()
}

/// Ответ вызова из записи: {"result": hex} → байты, {"error": …} → откат с текстом.
fn piece(v: &Value) -> Result<Bytes, String> {
    match v.get("result").and_then(Value::as_str) {
        Some(h) => Bytes::from_str(h).map_err(|e| e.to_string()),
        None => Err(v.get("error").map(Value::to_string).unwrap_or_default()),
    }
}

/// Ответы на вызовы бота по их calldata. Паника — в записи нет вызова бота.
fn answers(p: &Value, calls: &[(Address, Bytes)]) -> Vec<Result<Bytes, String>> {
    calls
        .iter()
        .map(|(_, d)| {
            let key = format!("{d}");
            piece(p["calls"].get(&key).unwrap_or_else(|| panic!("в записи нет вызова бота {key}")))
        })
        .collect()
}

/// Код из записи {"code": hex}. Паника — нет кода.
fn code(v: &Value) -> Vec<u8> {
    Bytes::from_str(v["code"].as_str().expect("нет кода")).unwrap().to_vec()
}

/// Код (или его начало) совпадает с ожидаемым: длина и keccak256. Принимает: код, (длина, хеш без 0x), целиком ли.
fn same(code: &[u8], want: (usize, &str), whole: bool) -> bool {
    let ok_len = if whole { code.len() == want.0 } else { code.len() >= want.0 };
    ok_len && format!("{:x}", keccak256(&code[..want.0.min(code.len())])) == want.1
}

/// Сверка лесенки get_dy пула с нашей формулой. Возвращает: число сверенных сумм.
fn ladder(name: &str, pool: Address, p: &Value, data: &CryptoData) -> usize {
    let mut n = 0;
    for r in p["get_dy"].as_array().unwrap() {
        let i = r["i"].as_u64().unwrap() as usize;
        let j = r["j"].as_u64().unwrap() as usize;
        let dx = U256::from_str(r["dx"].as_str().unwrap()).unwrap();
        let want = piece(&r["get_dy"]).ok().and_then(|b| quoter_crypto::decode(&b));
        let ours = crypto_swap::exchange_out(data, i, j, dx);
        assert_eq!(ours, want, "{name} {pool}: i={i} j={j} dx={dx}");
        n += 1;
    }
    n
}

/// Пулы NG: формула по MATH() (crypto_variant::variant), только версии из записанных списков; хотя бы один сверенный
/// пул каждой математики. Принимает: имя файла, число монет.
fn check_ng(name: &str, n: usize) {
    let d = doc(name);
    let now = d["timestamp"].as_u64().unwrap();
    let maths: Vec<Address> = d["maths"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| Address::from_str(m.as_str().unwrap()).unwrap())
        .collect();
    // Twocrypto: maths[0] — v2.1.0 (dex.curve_crypto_math), maths[1] — v2.0.0; Tricrypto — все в dex.curve_crypto_math.
    let (ng, v200) = if n == 2 {
        (vec![maths[0]], vec![maths[1]])
    } else {
        (maths.clone(), Vec::new())
    };
    let mut per_math = vec![0usize; maths.len()];
    for p in d["pools"].as_array().unwrap() {
        let pool = Address::from_str(p["pool"].as_str().unwrap()).unwrap();
        let got = answers(p, &load_crypto::calls(pool, n));
        let raw = load_crypto::Raw { pool, n, got, now };
        let (_, mut data, math) =
            load_crypto::decode(&raw).unwrap_or_else(|w| panic!("{pool}: {w}"));
        let Ok(v) = crypto_variant::variant(n, math, &v200, &ng) else {
            continue;
        };
        data.math = v;
        if ladder(name, pool, p, &data) > 0 {
            per_math[maths.iter().position(|m| *m == math).unwrap()] += 1;
        }
    }
    for (m, k) in maths.iter().zip(&per_math) {
        assert!(*k > 0, "{name}: нет ни одного сверенного пула математики {m}");
    }
}

/// Twocrypto-NG v2.1.0 и v2.0.0: наш выход = get_dy пула на каждой записанной сумме.
#[test]
#[ignore = "нет записи: tools/arbitrum_record/curve_crypto.py"]
fn twocrypto_matches_get_dy() {
    check_ng("twocrypto.json", 2);
}

/// Tricrypto-NG: наш выход = get_dy пула на каждой записанной сумме.
#[test]
#[ignore = "нет записи: tools/arbitrum_record/curve_crypto.py"]
fn tricrypto_matches_get_dy() {
    check_ng("tricrypto.json", 3);
}

/// Код Twocrypto-NG на узле = код из исходника (математика, реализации, начало кода пулов по их MATH()).
#[test]
#[ignore = "нет записи: tools/arbitrum_record/curve_crypto.py"]
fn twocrypto_code_matches_source() {
    let d = doc("twocrypto.json");
    let math = |k: usize| d["maths"][k].as_str().unwrap().to_string();
    assert!(same(&code(&d["code_of"][math(0)]), MATH_V210, true), "код math v2.1.0");
    assert!(same(&code(&d["code_of"][math(1)]), MATH_V200, true), "код math v2.0.0");
    let impls: Vec<Vec<u8>> = d["code_of"].as_object().unwrap().values().map(code).collect();
    assert!(impls.iter().any(|c| same(c, AMM_V210, true)), "нет кода реализации v2.1.0");
    assert!(impls.iter().any(|c| same(c, AMM_V200, true)), "нет кода реализации v2.0.0");
    for p in d["pools"].as_array().unwrap() {
        let c = code(&p["code"]);
        let pool = p["pool"].as_str().unwrap();
        let known = same(&c, POOL_V210, false) || same(&c, POOL_V200, false);
        assert!(known, "{pool}: код не из исходника");
    }
}

/// Пулы v1: разбор load_crypto_v1 (decimals из записи), наш выход = get_dy пула. Принимает: имя файла, число монет.
fn check_v1(name: &str, n: usize) {
    let d = doc(name);
    let now = d["timestamp"].as_u64().unwrap();
    let mut checked = 0;
    for p in d["pools"].as_array().unwrap() {
        let pool = Address::from_str(p["pool"].as_str().unwrap()).unwrap();
        let got = answers(p, &load_crypto_v1::calls(pool, n));
        let decimals: Vec<u8> = p["decimals"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as u8)
            .collect();
        let raw = load_crypto_v1::Raw { pool, n, got, now, decimals };
        let (_, data) = load_crypto_v1::decode(&raw).unwrap_or_else(|w| panic!("{pool}: {w}"));
        if data.halted {
            continue;
        }
        if n == 2 {
            let t = proxy_target(&code(&p["code"])).expect("код пула v1 — не прокси EIP-1167");
            let templates = d["templates"].as_array().unwrap();
            assert!(templates.iter().any(|x| Address::from_str(x.as_str().unwrap()).unwrap() == t));
        }
        checked += ladder(name, pool, p, &data);
    }
    assert!(checked > 0, "{name}: нет ни одной сверенной суммы");
}

/// Фабрика crypto v1: наш выход = get_dy пула; код пула — прокси на шаблон из журнала развёртывания.
#[test]
#[ignore = "нет записи: tools/arbitrum_record/curve_crypto.py"]
fn crypto_v1_matches_get_dy() {
    check_v1("crypto_v1.json", 2);
}

/// Tricrypto 2021: наш выход = get_dy пула.
#[test]
#[ignore = "нет записи: tools/arbitrum_record/curve_crypto.py"]
fn tricrypto_v1_matches_get_dy() {
    check_v1("tricrypto_v1.json", 3);
}
