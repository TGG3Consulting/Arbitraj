//! Файл: tests/bsc_dex_pmm_ladder.rs
//! Что делает: «лесенка цен пула» WooFi (решение 122 F, 3; quote/pmm_ladder.rs) без сети: (1) арифметика лесенки —
//! в точках выход точный, между точками средняя цена линейна, ниже первой точки — средняя цена первой, выше последней —
//! неисполнимо; глубина — граница по средней цене внутри отрезка, лесенка кончилась раньше полосы — full = false;
//! (2) на ЖИВОЙ записи узла — точки лесенки и промежуточные ответы квотера query того же блока: в точках выход
//! совпадает, между точками — печать отклонения интерполяции (запись — tests/README.md; пока её нет — #[ignore]).
//! Кто вызывает: cargo test (живой — cargo test --test bsc_dex_pmm_ladder -- --ignored, когда запись есть).
//! От чего зависит: arb::venues::bsc_dex::quote::pmm_ladder, arb::core::types::LadderPoint.

use alloy_primitives::U256;
use serde_json::Value;

use arb::core::types::LadderPoint;
use arb::venues::bsc_dex::quote::pmm_ladder::{depth, out};

fn p(i: u64, o: u64) -> LadderPoint {
    LadderPoint {
        amount_in: U256::from(i),
        amount_out: U256::from(o),
    }
}

/// Точки: средняя цена 2.0 при 100, 1.9 при 200, 1.5 при 400.
fn ladder() -> Vec<LadderPoint> {
    vec![p(100, 200), p(200, 380), p(400, 600)]
}

/// В точках — точный выход; между — средняя цена линейно (при 150: 1.95 → 292); выше последней — None.
#[test]
fn exact_at_points_linear_between() {
    let l = ladder();
    assert_eq!(out(&l, U256::from(100u64)), Some(U256::from(200u64)));
    assert_eq!(out(&l, U256::from(200u64)), Some(U256::from(380u64)));
    assert_eq!(out(&l, U256::from(400u64)), Some(U256::from(600u64)));
    assert_eq!(out(&l, U256::from(150u64)), Some(U256::from(292u64)));
    // 300: 1.9 → 1.5 на полпути = 1.7 → 510.
    assert_eq!(out(&l, U256::from(300u64)), Some(U256::from(510u64)));
    assert_eq!(out(&l, U256::from(401u64)), None);
    // Ниже первой точки — средняя цена первой точки (2.0).
    assert_eq!(out(&l, U256::from(50u64)), Some(U256::from(100u64)));
    assert_eq!(out(&[], U256::from(1u8)), None);
}

/// Глубина: граница средней цены 2.0 × 0.8 = 1.6 — на отрезке 200–400 (1.9 → 1.5) при 350; полоса шире лесенки —
/// последняя точка и full = false.
#[test]
fn depth_by_average_price() {
    let l = ladder();
    let e18 = |x: f64| U256::from((x * 1e18) as u128);
    let (i, o, full) = depth(&l, e18(0.8)).unwrap();
    assert_eq!(i, U256::from(350u64));
    assert_eq!(o, U256::from(560u64));
    assert!(full);
    let (i, _, full) = depth(&l, e18(0.5)).unwrap();
    assert_eq!(i, U256::from(400u64));
    assert!(!full);
}

/// Слово hex или десятичное → U256.
fn num(v: &Value) -> U256 {
    let s = v.as_str().expect("строка");
    match s.strip_prefix("0x") {
        Some(h) => U256::from_be_slice(&alloy_primitives::hex::decode(h).unwrap()),
        None => s.parse().unwrap(),
    }
}

/// Запись tests/data/woofi/ladder.json: {block, pairs: [{base, quote_in: [{in, out}], base_in: [{in, out}],
/// between_quote_in: [{in, out}], between_base_in: [{in, out}]}]} — все out — ответы query того же блока.
#[test]
#[ignore = "нет записи tests/data/woofi/ladder.json — запрос в tests/README.md (решение 122 F, 3)"]
fn live_ladder_exact_at_points() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/woofi/ladder.json");
    let rec: Value = serde_json::from_str(&std::fs::read_to_string(path).expect("запись")).unwrap();
    let points = |v: &Value| -> Vec<LadderPoint> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|x| LadderPoint {
                amount_in: num(&x["in"]),
                amount_out: num(&x["out"]),
            })
            .collect()
    };
    for pair in rec["pairs"].as_array().expect("pairs") {
        for (side, mid) in [
            ("quote_in", "between_quote_in"),
            ("base_in", "between_base_in"),
        ] {
            let l = points(&pair[side]);
            for x in &l {
                assert_eq!(
                    out(&l, x.amount_in),
                    Some(x.amount_out),
                    "{} {side}",
                    pair["base"]
                );
            }
            for m in points(&pair[mid]) {
                let got = out(&l, m.amount_in).expect("в пределах лесенки");
                let (a, b) = (got.to::<u128>() as f64, m.amount_out.to::<u128>() as f64);
                eprintln!(
                    "{} {side} {}: интерполяция {got} / квотер {} ({:+.4}%)",
                    pair["base"],
                    m.amount_in,
                    m.amount_out,
                    (a - b) / b * 100.0
                );
            }
        }
    }
}
