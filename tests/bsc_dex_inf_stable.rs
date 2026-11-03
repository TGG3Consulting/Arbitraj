//! Файл: tests/bsc_dex_inf_stable.rs
//! Что делает: PancakeSwap Infinity Stable (решение «122 (Infinity Stable)») без сети и без моков: (1) вид
//! pancake_infinity_stable — протокол, вид ядра StableSwap, в графе, исполним Universal Router (tradable, не форк),
//! без шага исполнителя; (2) селекторы геттеров и квотера крючка — по подписям verified-кода; (3) свойства порта
//! math/stableswap_ng.rs: динамическая комиссия равна fee в точке привязки и растёт вне её, D сбалансированного пула
//! = сумме, обмен по NG при offpeg ≤ 1e10 расходится с шаблоном A_PRECISION не больше округления; (4) сверка своей
//! математики _exchange_received с квотером крючка get_dy на ЖИВОЙ записи узла — ждёт записи
//! tests/data/infinity_stable/responses.json (запрос — tests/README.md), пока записи нет — #[ignore].
//! Кто вызывает: cargo test (живой — cargo test --test bsc_dex_inf_stable -- --ignored, когда запись есть).
//! От чего зависит: arb::venues::bsc_dex (families, math::stableswap_ng, math::stableswap_v1, pools::abi_inf_stable).

use alloy::sol_types::SolCall;
use alloy_primitives::U256;
use serde_json::Value;

use arb::core::types::PoolKind;
use arb::venues::bsc_dex::math::stableswap_ng as ng;
use arb::venues::bsc_dex::math::stableswap_v1::{self, Template};
use arb::venues::bsc_dex::pools::abi_inf_stable::IInfStableHook as H;
use arb::venues::bsc_dex::pools::families::{self, Proto};

fn e18(n: u64) -> U256 {
    U256::from(n) * U256::from(10u8).pow(U256::from(18u8))
}

fn e10() -> U256 {
    U256::from(10u64.pow(10))
}

/// Вид: StableSwap, в графе, tradable (Universal Router своей DEX), не форк, шага исполнителя нет.
#[test]
fn family_flags() {
    let f = families::make("x", "pancake_infinity_stable").unwrap();
    assert_eq!(
        (f.proto, f.kind),
        (Proto::InfinityStable, PoolKind::StableSwap)
    );
    assert_eq!((f.graphable, f.tradable, f.fork), (true, true, false));
    assert!(f.executor_kind.is_none());
    assert!(f.proto.is_stable());
}

/// Селекторы = keccak подписей геттеров verified-кода CLStableSwapHook (abi_inf_stable.rs) — те же, что в запросе
/// записи tests/README.md.
#[test]
fn hook_selectors() {
    let hex = |s: [u8; 4]| alloy_primitives::hex::encode(s);
    assert_eq!(hex(H::get_dyCall::SELECTOR), "5e0d443f");
    assert_eq!(hex(H::coinsCall::SELECTOR), "c6610657");
    assert_eq!(hex(H::balancesCall::SELECTOR), "4903b0d1");
    assert_eq!(hex(H::feeCall::SELECTOR), "ddca3f43");
    assert_eq!(hex(H::A_preciseCall::SELECTOR), "76a2f0f0");
    assert_eq!(hex(H::stored_ratesCall::SELECTOR), "fd0684b1");
    assert_eq!(hex(H::offpeg_fee_multiplierCall::SELECTOR), "8edfdd5f");
    assert_eq!(hex(H::N_COINSCall::SELECTOR), "29357750");
    assert_eq!(hex(H::pool_managerCall::SELECTOR), "5592ab3e");
}

/// _dynamic_fee: offpeg ≤ 1e10 — ровно fee; в точке привязки (xpi = xpj) — ровно fee; вне привязки — больше fee и не
/// больше offpeg·fee/1e10.
#[test]
fn dynamic_fee_shape() {
    let fee = U256::from(1_000_000u64); // 0,01 %
    let m = U256::from(2u8) * e10(); // offpeg 2
    assert_eq!(ng::dynamic_fee(e18(1), e18(9), fee, e10()), Some(fee));
    assert_eq!(ng::dynamic_fee(e18(5), e18(5), fee, m), Some(fee));
    let off = ng::dynamic_fee(e18(1), e18(9), fee, m).unwrap();
    assert!(off > fee && off <= m * fee / e10(), "{off}");
}

/// get_D сбалансированного пула = сумме балансов (неподвижная точка формулы NG).
#[test]
fn d_of_balanced_pool_is_sum() {
    let xp = [e18(1_000_000), e18(1_000_000)];
    assert_eq!(
        ng::get_d(&xp, U256::from(200u64 * 100)),
        Some(e18(2_000_000))
    );
}

/// При offpeg ≤ 1e10 обмен NG = шаблон A_PRECISION (255 раундов, откат) с точностью до округления D_P (≤ 2 единицы).
#[test]
fn ng_matches_aprec_template_without_offpeg() {
    let t = Template {
        amp_precision: U256::from(100u8),
        max_rounds: 255,
        strict: true,
    };
    let balances = [e18(3_000_000), e18(2_500_000)];
    let rates = [e18(1), e18(1)];
    let (amp, fee) = (U256::from(1000u64 * 100), U256::from(4_000_000u64));
    for dx in [e18(1), e18(10_000), e18(500_000)] {
        let a = ng::exchange_received(0, 1, dx, &balances, &rates, amp, fee, e10())
            .unwrap()
            .0;
        let b = stableswap_v1::exchange_out_t(0, 1, dx, &balances, &rates, amp, fee, &t).unwrap();
        let d = if a > b { a - b } else { b - a };
        assert!(d <= U256::from(2u8), "dx {dx}: {a} vs {b}");
    }
}

/// Слово hex-ответа eth_call → U256.
fn word(v: &Value) -> U256 {
    let b = alloy_primitives::hex::decode(v.as_str().expect("строка").trim_start_matches("0x"))
        .unwrap();
    U256::from_be_slice(&b)
}

/// Сверка с квотером крючка по записи (формат — tests/README.md): {block, pools: [{hook, balances: ["<result>"×2],
/// stored_rates: "<result>", A_precise, fee, offpeg_fee_multiplier, quotes: [{i, j, dx: "<десятичное>",
/// out: "<result get_dy>"}]}]}. Выход должен совпасть точно.
#[test]
#[ignore = "нет записи tests/data/infinity_stable/responses.json — запрос в tests/README.md (решение «122 (Infinity Stable)»)"]
fn ng_math_matches_live_get_dy() {
    let path = format!(
        "{}/tests/data/infinity_stable/responses.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let rec: Value = serde_json::from_str(&std::fs::read_to_string(path).expect("запись")).unwrap();
    for p in rec["pools"].as_array().expect("pools") {
        let balances: Vec<U256> = p["balances"].as_array().unwrap().iter().map(word).collect();
        let raw = alloy_primitives::hex::decode(
            p["stored_rates"].as_str().unwrap().trim_start_matches("0x"),
        )
        .unwrap();
        let rates = H::stored_ratesCall::abi_decode_returns(&raw).unwrap();
        let (amp, fee, offpeg) = (
            word(&p["A_precise"]),
            word(&p["fee"]),
            word(&p["offpeg_fee_multiplier"]),
        );
        for q in p["quotes"].as_array().unwrap() {
            let (i, j) = (
                q["i"].as_u64().unwrap() as usize,
                q["j"].as_u64().unwrap() as usize,
            );
            let dx: U256 = q["dx"].as_str().unwrap().parse().unwrap();
            let got = ng::exchange_received(i, j, dx, &balances, &rates, amp, fee, offpeg)
                .unwrap()
                .0;
            assert_eq!(got, word(&q["out"]), "крючок {} {i}→{j} dx {dx}", p["hook"]);
        }
    }
}
