//! Файл: tests/bsc_dex_solidly_fee_273.rs
//! Что делает: решение 273 — математика комиссии Solidly по правилу, без сети и без моков, на ЖИВОЙ записи узла BSC
//! (tests/data/solidly_fee_2026-11-14, один блок 133593628, README там же):
//! (1) математика бота (правило комиссии math/solidly_fee.rs + формулы пары с нулевой комиссией — ровно путь
//!   quote/pool_quote.rs) совпадает с getAmountOut пары ДО WEI на 9 пулах трёх DEX (usdfi — делитель, в том числе
//!   fee 333 и 2500, где правила расходятся; veplus, magicfox — числитель над 10000), volatile и stable, обе стороны;
//! (2) правило «числитель над 10000» на парах usdfi НЕ совпадает (без решения 273 пулы считались бы неверно);
//! (3) «1 в 1» с прежним путём при правиле «числитель над 10000» на всех записанных образцах veplus/magicfox:
//!   (а) solidly_v1_math (THENA V1-тип) — те же числа, что прежние функции с комиссией внутри;
//!   (б) ветка swap_solidly::get_amount_out (stable Velodrome/Topaz-тип) — вход после комиссии и формула с нулевой
//!   комиссией = прежний вызов с комиссией внутри, на стабильных пулах записи (ревью 273);
//!   (в) обратный расчёт amount_in волатильного пула (v2::amount_in) — доля keep_ratio = прежняя (10000−fee)/10000.
//! Настройки, ограничения, Sync, чтение записей обоих форматов — tests/bsc_dex_solidly_fee_273_cfg.rs.
//! Кто вызывает: cargo test --test bsc_dex_solidly_fee_273.
//! От чего зависит: tests/solidly273_common, arb::venues::bsc_dex (math::{solidly_fee, solidly_v1_math,
//! swap_solidly, v2}), arb::core::types.

mod solidly273_common;

use alloy_primitives::U256;

use arb::core::types::SolidlyFee;
use arb::venues::bsc_dex::math::{solidly_fee, solidly_v1_math, swap_solidly, v2};
use solidly273_common::{NUM, fee_of, load, ours, samples, u};

/// Резервы и decimals пула записи по направлению: (вход, выход, dec входа, dec выхода).
fn sides(p: &serde_json::Value, token_in: &str) -> (U256, U256, U256, U256) {
    let m = &p["metadata"];
    if token_in == m["t0"].as_str().unwrap() {
        (u(&m["r0"]), u(&m["r1"]), u(&m["dec0"]), u(&m["dec1"]))
    } else {
        (u(&m["r1"]), u(&m["r0"]), u(&m["dec1"]), u(&m["dec0"]))
    }
}

/// (1) До wei с getAmountOut цепи на всех 36 образцах; metadata и getReserves одного блока совпадают.
#[test]
fn bot_math_equals_get_amount_out_to_wei() {
    let s = load("state.json");
    assert_eq!(s["block"], 133593628);
    let all = samples(&s);
    assert_eq!(all.len(), 36, "9 пулов × 2 стороны × 2 суммы");
    let (mut st, mut vol) = (0, 0);
    for (dex, d, p, a, t, want) in &all {
        let m = &p["metadata"];
        assert_eq!(p["getReserves"][0], m["r0"]);
        assert_eq!(p["getReserves"][1], m["r1"]);
        let fee = fee_of(dex, d, p);
        let got = ours(&fee, p, *a, t).unwrap();
        assert_eq!(got, *want, "{dex} {} вход {a} {t}", p["pool"]);
        if m["st"].as_bool().unwrap() {
            st += 1
        } else {
            vol += 1
        }
    }
    assert!(st >= 12 && vol >= 12, "и stable, и volatile: {st}/{vol}");
}

/// (2) У usdfi правило «числитель над 10000» дало бы другой выход (fee 333 и 2500 — заведомо).
#[test]
fn usdfi_numerator_rule_does_not_match_chain() {
    let s = load("state.json");
    for (dex, d, p, a, t, want) in samples(&s).iter().filter(|x| x.0 == "usdfi") {
        let f = fee_of(dex, d, p);
        let wrong = SolidlyFee {
            value: f.value,
            rule: NUM,
        };
        assert_ne!(
            ours(&wrong, p, *a, t),
            Some(*want),
            "{} fee {}",
            p["pool"],
            f.value
        );
    }
    let fees: Vec<u32> = s["dex"]["usdfi"]["pools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| u(&p["fee"]).to::<u32>())
        .collect();
    assert_eq!(fees, vec![2500, 333, 10000]);
}

/// (3) Правило «числитель над 10000» — те же числа, что прежние функции с комиссией внутри (путь до решения 273).
#[test]
fn numerator_10000_is_byte_identical_to_previous_path() {
    let s = load("state.json");
    let mut n = 0;
    for (dex, d, p, a, t, want) in samples(&s).iter().filter(|x| x.0 != "usdfi") {
        let f = fee_of(dex, d, p);
        let m = &p["metadata"];
        let zfo = t == m["t0"].as_str().unwrap();
        let (r_in, r_out, d_in, d_out) = if zfo {
            (u(&m["r0"]), u(&m["r1"]), u(&m["dec0"]), u(&m["dec1"]))
        } else {
            (u(&m["r1"]), u(&m["r0"]), u(&m["dec1"]), u(&m["dec0"]))
        };
        let old = if m["st"].as_bool().unwrap() {
            solidly_v1_math::stable_amount_out(*a, r_in, r_out, d_in, d_out, U256::from(f.value))
        } else {
            solidly_v1_math::volatile_amount_out(*a, r_in, r_out, U256::from(f.value))
        };
        assert_eq!(ours(&f, p, *a, t), old);
        assert_eq!(old, Some(*want));
        n += 1;
    }
    assert_eq!(n, 24);
}

/// (3б) Ветка swap_solidly::get_amount_out (stable Velodrome/Topaz-тип, quote/pool_quote.rs при proto ≠ SolidlyV1):
/// вход после комиссии по правилу «числитель над 10000» и формула с нулевой комиссией = прежний вызов с комиссией
/// внутри — на всех стабильных образцах veplus/magicfox записи.
#[test]
fn swap_solidly_stable_branch_is_byte_identical() {
    let s = load("state.json");
    let mut n = 0;
    for (dex, d, p, a, t, _) in samples(&s).iter().filter(|x| x.0 != "usdfi") {
        if !p["metadata"]["st"].as_bool().unwrap() {
            continue;
        }
        let f = fee_of(dex, d, p);
        let (r_in, r_out, d_in, d_out) = sides(p, t);
        let old = swap_solidly::get_amount_out(*a, r_in, r_out, d_in, d_out, U256::from(f.value));
        let net = solidly_fee::net_in(*a, &f).unwrap();
        let new = swap_solidly::get_amount_out(net, r_in, r_out, d_in, d_out, U256::ZERO);
        assert!(old.is_some(), "{dex} {} вход {a}", p["pool"]);
        assert_eq!(new, old, "{dex} {} вход {a} {t}", p["pool"]);
        n += 1;
    }
    assert_eq!(
        n, 16,
        "4 стабильных пула veplus/magicfox × 2 стороны × 2 суммы"
    );
}

/// (3в) Обратный расчёт amount_in волатильного пула: доля keep_ratio при «числителе над 10000» = прежняя дробь
/// (10000−fee)/10000 — тот же amount_in на выходах записи (getAmountOut цепи) всех волатильных пулов veplus/magicfox.
#[test]
fn volatile_amount_in_is_byte_identical() {
    let s = load("state.json");
    // Прежний делитель дроби (10000) — fee_denominator(), одно число на проект.
    let den = swap_solidly::fee_denominator();
    let mut n = 0;
    for (dex, d, p, a, t, want) in samples(&s).iter().filter(|x| x.0 != "usdfi") {
        if p["metadata"]["st"].as_bool().unwrap() {
            continue;
        }
        let f = fee_of(dex, d, p);
        let (r_in, r_out, _, _) = sides(p, t);
        let old_ratio = (den - U256::from(f.value), den);
        assert_eq!(solidly_fee::keep_ratio(&f), Some(old_ratio));
        let old = v2::amount_in(*want, r_in, r_out, old_ratio);
        let new = v2::amount_in(*want, r_in, r_out, solidly_fee::keep_ratio(&f).unwrap());
        assert!(old.is_some(), "{dex} {} выход {want}", p["pool"]);
        assert_eq!(
            new, old,
            "{dex} {} выход {want} {t} (вход записи {a})",
            p["pool"]
        );
        n += 1;
    }
    assert_eq!(
        n, 8,
        "2 волатильных пула veplus/magicfox × 2 стороны × 2 суммы"
    );
}
