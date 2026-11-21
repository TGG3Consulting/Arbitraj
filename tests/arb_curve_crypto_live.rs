//! Файл: tests/arb_curve_crypto_live.rs
//! Что проверяет: решения 442, 453 — crypto-пулы Curve NG Arbitrum One (Twocrypto-NG v2.1.0 и v2.0.0, Tricrypto-NG) на
//! РЕАЛЬНЫХ ответах своего узла Arbitrum, записанных на одном блоке скриптом tools/arbitrum_record/curve_crypto.py
//! (tests/data/arbitrum_curve_crypto_<дата>/twocrypto.json, tricrypto.json); пулы первого поколения —
//! tests/arb_curve_crypto_v1_live.rs (разбито по ревью 453):
//! 1. Ответы берутся по calldata вызовов бота (pools/load_crypto.rs calls()) — порядок и данные вызовов те же, что у
//!    бота (ревью 442, п. 8); разбор — decode бота.
//! 2. Наша математика (math/crypto_swap.rs exchange_out: формула версии пула, рампа — newton_D) на записанном состоянии
//!    даёт ТОТ ЖЕ ответ, что get_dy пула, на каждой сумме лесенки в обе стороны; откат get_dy — у нас None. Пулы вне
//!    списков математики (MATH() не из записанных `maths`) и версии не из списка не сверяются (как verify_crypto.rs), но
//!    сверенным должен быть хотя бы один пул каждой математики (ревью 442, п. 1).
//! 3. Код (eth_getCode) математики и реализаций Twocrypto-NG — ровно код, собранный из исходника (Vyper 0.3.10,
//!    twocrypto-ng d612fde — v2.0.0, 99b5192 — v2.1.0; хеши ниже), код пулов начинается кодом своей реализации.
//!
//! Запись tests/data/arbitrum_curve_crypto_2026-11-20 — метки #[ignore] сняты (решение 461; тесты — только на
//! записанных ответах, CLAUDE.md).
//! Кто вызывает: cargo test.
//! От чего зависит: tests/curve_crypto_common (запись, лесенка), arb::venues::bsc_dex::pools::{load_crypto,
//! crypto_variant}. Моков нет.

mod curve_crypto_common;

use std::str::FromStr;

use alloy_primitives::Address;

use arb::venues::bsc_dex::pools::{crypto_variant, load_crypto};
use curve_crypto_common::{answers, code, doc, ladder, same};

/// Хеши кода Twocrypto-NG, собранного из исходника Vyper 0.3.10 (CREATE2-адреса сборки совпали с deployments.yaml):
/// (имя, длина, keccak256). Математика — код целиком; реализация (blueprint) — 0xFE7100 + код развёртывания; пул —
/// начало кода (runtime реализации, за ним — immutables).
const MATH_V210: (usize, &str) = (
    10137,
    "70d105ee0e9b857244d87521bbcef34cfb4cbd600592af61bbc7c76180648d61",
);
const MATH_V200: (usize, &str) = (
    9964,
    "bc931b24ccca3aae2cd24bf83022386ae21d4c96af2f7c7f57a91c743b8352e3",
);
const AMM_V210: (usize, &str) = (
    18837,
    "cb85feb205dc33b1704e8b492cb1d22c4b6728e23558123618b30216a15dea62",
);
const AMM_V200: (usize, &str) = (
    20742,
    "29fefecf08c8794a6a8af8d5382373a2ac7364f81e25f4950894dbaed4eef22d",
);
const POOL_V210: (usize, &str) = (
    17875,
    "e40fd9046a6204d8e7985312d45a721dae1ff759dd9d91358f06024c5ec4bc37",
);
const POOL_V200: (usize, &str) = (
    19738,
    "fe2e067b2dffd2dc7f5af5fe2dc58ab997acbceee805e562e569237ecd323a74",
);

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
        assert!(
            *k > 0,
            "{name}: нет ни одного сверенного пула математики {m}"
        );
    }
}

/// Twocrypto-NG v2.1.0 и v2.0.0: наш выход = get_dy пула на каждой записанной сумме.
#[test]
fn twocrypto_matches_get_dy() {
    check_ng("twocrypto.json", 2);
}

/// Tricrypto-NG: наш выход = get_dy пула на каждой записанной сумме.
#[test]
fn tricrypto_matches_get_dy() {
    check_ng("tricrypto.json", 3);
}

/// Код Twocrypto-NG на узле = код из исходника (математика, реализации, начало кода пулов по их MATH()).
#[test]
fn twocrypto_code_matches_source() {
    let d = doc("twocrypto.json");
    let math = |k: usize| d["maths"][k].as_str().unwrap().to_string();
    assert!(
        same(&code(&d["code_of"][math(0)]), MATH_V210, true),
        "код math v2.1.0"
    );
    assert!(
        same(&code(&d["code_of"][math(1)]), MATH_V200, true),
        "код math v2.0.0"
    );
    let impls: Vec<Vec<u8>> = d["code_of"]
        .as_object()
        .unwrap()
        .values()
        .map(code)
        .collect();
    assert!(
        impls.iter().any(|c| same(c, AMM_V210, true)),
        "нет кода реализации v2.1.0"
    );
    assert!(
        impls.iter().any(|c| same(c, AMM_V200, true)),
        "нет кода реализации v2.0.0"
    );
    for p in d["pools"].as_array().unwrap() {
        let c = code(&p["code"]);
        let pool = p["pool"].as_str().unwrap();
        let known = same(&c, POOL_V210, false) || same(&c, POOL_V200, false);
        assert!(known, "{pool}: код не из исходника");
    }
}
