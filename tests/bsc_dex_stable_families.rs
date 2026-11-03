//! Файл: tests/bsc_dex_stable_families.rs
//! Что делает: семейства решения 122 F, 3 без сети и без моков — PancakeSwap StableSwap, Lista StableSwap, Saddle
//! (Nerve), WooFi WooPPV2: (1) таблица сопоставления → протокол, вид ядра, в графе, без своего пути, без шага
//! исполнителя (у контракта нет подходящего вида шага), нужные настройки; (2) свойства общей математики шаблонов
//! (stableswap_v1::Template): A_PRECISION = 100 с A·100 даёт ровно то же, что сырой A (формулы Lista/Saddle сводятся к
//! Curve 2020 точно); рампа A Lista (get_A); проверка цены Lista после обмена (stable_guard): малый обмен в
//! сбалансированном пуле проходит, обмен, уводящий цену за порог, — нет; (3) сверка своей математики с квотером пула
//! на ЖИВОЙ записи узла — ждёт записей tests/data/{pancake_stable,lista_stable,nerve}/responses.json (запрос —
//! tests/README.md), пока записей нет — #[ignore]. WooFi своей математики не имеет (развёрнутый код закрыт).
//! Кто вызывает: cargo test (живые — cargo test --test bsc_dex_stable_families -- --ignored, когда записи есть).
//! От чего зависит: arb::venues::bsc_dex (families, exec_kind, math::stableswap_v1, math::stable_guard,
//! pools::load_stable_u::ramp), arb::core::types.

use alloy_primitives::U256;
use serde_json::Value;

use arb::core::types::{PoolKind, StableGuard};
use arb::venues::bsc_dex::math::stable_guard;
use arb::venues::bsc_dex::math::stableswap_v1::{self, Template, curve2020};
use arb::venues::bsc_dex::pools::families::{self, Proto};
use arb::venues::bsc_dex::pools::load_stable_u::ramp;

fn e18(n: u64) -> U256 {
    U256::from(n) * U256::from(10u8).pow(U256::from(18u8))
}

/// Шаблон A_PRECISION (Lista: 255 раундов без отката; Saddle — 256 с откатом).
fn aprec(max_rounds: u16, strict: bool) -> Template {
    Template {
        amp_precision: U256::from(100u8),
        max_rounds,
        strict,
    }
}

/// Виды: протокол, вид ядра StableSwap/OraclePmm, в графе, без своего пути и без шага исполнителя.
#[test]
fn families_flags() {
    for (kind, proto, pk) in [
        (
            "pancake_stableswap",
            Proto::PancakeStable,
            PoolKind::StableSwap,
        ),
        ("lista_stableswap", Proto::ListaStable, PoolKind::StableSwap),
        (
            "saddle_stableswap",
            Proto::SaddleStable,
            PoolKind::StableSwap,
        ),
        ("woofi_v2", Proto::WooPP, PoolKind::OraclePmm),
    ] {
        let f = families::make("x", kind).unwrap();
        assert_eq!((f.proto, f.kind), (proto, pk), "{kind}");
        assert_eq!(
            (f.graphable, f.tradable, f.fork),
            (true, false, false),
            "{kind}"
        );
        assert_eq!(f.executor_kind, None, "{kind}: шага исполнителя нет");
    }
}

/// A·100 при точности 100 = сырой A при точности 1 (дословно: деления на AP точны), для D и для обмена.
#[test]
fn a_precision_reduces_to_curve2020() {
    let rates = vec![e18(1), e18(1)];
    let balances = vec![e18(1_000_000), e18(700_000)];
    let xp = stableswap_v1::xp(&balances, &rates).unwrap();
    for a in [20u64, 100, 5000] {
        let raw = stableswap_v1::get_d_t(&xp, U256::from(a), &curve2020()).unwrap();
        let prec = stableswap_v1::get_d_t(&xp, U256::from(a * 100), &aprec(255, false)).unwrap();
        assert_eq!(raw, prec, "D, A = {a}");
        let fee = U256::from(1_000_000u64); // 0,01% над 10^10
        let dx = e18(10_000);
        let r = stableswap_v1::exchange_out_t(
            0,
            1,
            dx,
            &balances,
            &rates,
            U256::from(a),
            fee,
            &curve2020(),
        );
        let p = stableswap_v1::exchange_out_t(
            0,
            1,
            dx,
            &balances,
            &rates,
            U256::from(a * 100),
            fee,
            &aprec(256, true),
        );
        assert_eq!(r, p, "обмен, A = {a}");
        assert!(r.unwrap() > U256::ZERO);
    }
}

/// Сбалансированный пул: D = сумма балансов (в 18 знаках) у обоих шаблонов.
#[test]
fn balanced_d_is_sum() {
    let xp = vec![e18(500), e18(500)];
    let d = stableswap_v1::get_d_t(&xp, U256::from(10_000u64), &aprec(256, true)).unwrap();
    let s = e18(1000);
    let diff = if d > s { d - s } else { s - d };
    assert!(diff <= U256::from(2u8), "D {d} ≠ {s}");
}

/// Рампа A (get_A Lista/PancakeSwap): до начала — initial, после конца — future, посередине — линейно.
#[test]
fn ramp_a() {
    let (a0, a1, t0, t1) = (
        U256::from(10_000u64),
        U256::from(20_000u64),
        U256::from(100u8),
        U256::from(200u8),
    );
    assert_eq!(ramp(a0, a1, t0, t1, U256::from(200u8)), Some(a1));
    assert_eq!(
        ramp(a0, a1, t0, t1, U256::from(150u8)),
        Some(U256::from(15_000u64))
    );
    assert_eq!(
        ramp(a1, a0, t0, t1, U256::from(150u8)),
        Some(U256::from(15_000u64))
    );
}

/// Проверка цены Lista: оракул 1 : 1, порог 3% — малый обмен проходит, обмен половины пула — нет.
#[test]
fn guard_small_passes_big_fails() {
    let rates = vec![e18(1), e18(1)];
    let balances = vec![e18(1_000_000), e18(1_000_000)];
    let g = StableGuard {
        admin_fee: U256::from(5_000_000_000u64),
        oracle: vec![e18(1), e18(1)],
        threshold: vec![U256::from(3u8) * U256::from(10u8).pow(U256::from(16u8)); 2],
    };
    let (amp, fee, t) = (
        U256::from(500u64 * 100),
        U256::from(1_000_000u64),
        aprec(255, false),
    );
    assert!(stable_guard::passes(
        0,
        1,
        e18(1_000),
        &balances,
        &rates,
        amp,
        fee,
        &t,
        &g
    ));
    assert!(!stable_guard::passes(
        0,
        1,
        e18(900_000),
        &balances,
        &rates,
        amp,
        fee,
        &t,
        &g
    ));
}

/// Слово hex-ответа eth_call → U256.
fn word(v: &Value) -> U256 {
    let b = alloy_primitives::hex::decode(v.as_str().expect("строка").trim_start_matches("0x"))
        .unwrap();
    U256::from_be_slice(&b)
}

/// Сверка с квотером пула по записи: {block, pools: [{pool, balances: [..], decimals: [..], amp: "<A в точности
/// шаблона>", fee, quotes: [{i, j, dx: "<десятичное>", out: "<result квотера>"}]}]}. Принимает: файл, шаблон.
fn live(file: &str, t: &Template, minus_fee_rounding: bool) {
    let path = format!("{}/tests/data/{file}", env!("CARGO_MANIFEST_DIR"));
    let rec: Value = serde_json::from_str(&std::fs::read_to_string(path).expect("запись")).unwrap();
    for p in rec["pools"].as_array().expect("pools") {
        let balances: Vec<U256> = p["balances"].as_array().unwrap().iter().map(word).collect();
        let rates: Vec<U256> = p["decimals"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| stableswap_v1::rate_for_decimals(d.as_u64().unwrap() as u8).unwrap())
            .collect();
        let (amp, fee) = (word(&p["amp"]), word(&p["fee"]));
        for q in p["quotes"].as_array().unwrap() {
            let (i, j) = (
                q["i"].as_u64().unwrap() as usize,
                q["j"].as_u64().unwrap() as usize,
            );
            let dx: U256 = q["dx"].as_str().unwrap().parse().unwrap();
            let got =
                stableswap_v1::exchange_out_t(i, j, dx, &balances, &rates, amp, fee, t).unwrap();
            let want = word(&q["out"]);
            // get_dy PancakeSwap/Lista снимает комиссию после перевода в единицы монеты — расхождение ≤ 1 единицы
            // при rates ≠ 1e18 (PancakeStableSwapTwoPool.sol L393-L404 против exchange L425-L466).
            let diff = if got > want { got - want } else { want - got };
            let tol = U256::from(u8::from(minus_fee_rounding));
            assert!(
                diff <= tol,
                "пул {} {i}→{j} dx {dx}: {got} ≠ {want}",
                p["pool"]
            );
        }
    }
}

#[test]
#[ignore = "нет записи tests/data/pancake_stable/responses.json — запрос в tests/README.md (решение 122 F, 3)"]
fn pancake_math_matches_live_get_dy() {
    live("pancake_stable/responses.json", &curve2020(), true);
}

#[test]
#[ignore = "нет записи tests/data/lista_stable/responses.json — запрос в tests/README.md (решение 122 F, 3)"]
fn lista_math_matches_live_get_dy() {
    live("lista_stable/responses.json", &aprec(255, false), true);
}

#[test]
#[ignore = "нет записи tests/data/nerve/responses.json — запрос в tests/README.md (решение 122 F, 3)"]
fn nerve_math_matches_live_calculate_swap() {
    live("nerve/responses.json", &aprec(256, true), false);
}
