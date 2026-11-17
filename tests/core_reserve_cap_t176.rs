//! Файл: tests/core_reserve_cap_t176.rs
//! Что проверяет: решение 390 (задача t176) — форма запаса на движение цены (core/reserve.rs keep, amount). Тест ЧИСТОЙ
//! ЛОГИКИ (перебор по сетке входов, без записей): запас — функция чисел k·σ·√T, выручки, доли и двух средних цен; на
//! записанных данных та же функция проверяется через полный расчёт в tests/core_price_reserve.rs, согласованность с
//! отсевом — tests/core_sieve_t162.rs (bound_covers_full_calc_large_reserve_t176) и tests/core_sieve_recorded_t162.rs.
//! Перебор: m = k·σ·√T ∈ {0, 10⁻⁶ … 10⁶}, выручка 0,01 … 10⁶, доля нет / 0 … 1, спред −50 % … 10⁶ %:
//! 1) 0 ≤ запас ≤ выручка (потерять больше выручки нельзя);
//! 2) запас ≥ выручка·(1 − keep(m)) — допущение отсева (bound_in.rs: κ_B = max(0, keep − r_B));
//! 3) keep ∈ [0, 1], не растёт с m; при m > ≈ 66 — 0 без паники;
//! 4) при малом m базовая часть совпадает с прежней m·выручка с точностью m²/2 выручки (ряд Тейлора e^(−m));
//! 5) часть доли = доля · (продажа − покупка) · проданное количество; прежняя (доля · спред · выручка) больше ровно
//!    в 1 + спред раз;
//! 6) новый запас никогда не больше прежнего max(m·выручка, доля · спред · выручка) — правка только снимает завышение.
//!
//! Печатает таблицу «было → стало» (доли выручки) для m = 0,001; 0,01; 0,1; 1; 5 и спреда 1 %; 25 %; 160 %; 10⁶ %
//! при доле 0,8.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::reserve. Моков нет.

use std::str::FromStr;

use arb::core::reserve::{amount, keep};
use rust_decimal::Decimal;

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

/// Прежняя форма (до решения 390): max(m·выручка, доля · спред · выручка), спред = (продажа − покупка) / покупка.
fn old(m: Decimal, p: Decimal, share: Option<Decimal>, buy: Decimal, sell: Decimal) -> Decimal {
    let base = m * p;
    match share {
        Some(s) if sell > buy => base.max(s * ((sell - buy) / buy) * p),
        _ => base,
    }
}

/// Допуск округления Decimal относительно выручки.
fn tol(p: Decimal) -> Decimal {
    p * d("0.0000000000000000000001")
}

#[test]
fn reserve_cap_grid() {
    let ms = [
        "0", "0.000001", "0.0001", "0.001", "0.01", "0.1", "0.5", "1", "2", "5", "10", "50", "65",
        "67", "100", "1000000",
    ];
    let ps = ["0.01", "1", "1000", "1000000"];
    let shares = [
        None,
        Some("0"),
        Some("0.16"),
        Some("0.39"),
        Some("0.8"),
        Some("1"),
    ];
    let spreads = ["-0.5", "0", "0.01", "0.25", "1.6", "10000"];
    let buy = d("0.0012637");
    let mut prev_keep = Decimal::ONE;
    for m in ms.map(d) {
        let kp = keep(m);
        assert!(
            (Decimal::ZERO..=Decimal::ONE).contains(&kp),
            "keep({m}) = {kp}"
        );
        assert!(kp <= prev_keep, "keep растёт: {m}");
        prev_keep = kp;
        if m < d("0.1") {
            // 0 ≤ m − (1 − e^(−m)) ≤ m²/2.
            let gap = m - (Decimal::ONE - kp);
            let hi = m * m / d("2") + tol(Decimal::ONE);
            assert!((-tol(Decimal::ONE)..=hi).contains(&gap), "{m}: {gap}");
        }
        for p in ps.map(d) {
            for share in shares.map(|s| s.map(d)) {
                for s in spreads.map(d) {
                    let sell = buy * (Decimal::ONE + s);
                    // k·σ·√T передаётся как k = m, σ = 1, √T = 1 — то же произведение.
                    let r = amount(m, Decimal::ONE, Decimal::ONE, p, share, buy, sell);
                    assert!(
                        (Decimal::ZERO..=p).contains(&r),
                        "m {m} p {p} доля {share:?} спред {s}: {r}"
                    );
                    let base = p * (Decimal::ONE - kp);
                    assert!(r >= base, "ниже базовой части: m {m} p {p} {share:?} {s}");
                    assert!(
                        r <= old(m, p, share, buy, sell) + tol(p),
                        "больше прежнего: m {m} p {p} {share:?} {s}"
                    );
                    if let Some(sh) = share
                        && s > Decimal::ZERO
                    {
                        let qty = p / sell;
                        let closure = sh * (sell - buy) * qty;
                        assert!(r + tol(p) >= closure, "ниже доли: m {m} p {p} {sh} {s}");
                        let old_part = sh * s * p;
                        assert!(
                            (old_part - closure * (Decimal::ONE + s)).abs()
                                <= tol(p) * (Decimal::ONE + s)
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn reserve_cap_table() {
    let (p, buy, share) = (Decimal::ONE, Decimal::ONE, d("0.8"));
    println!("Базовая часть, доля выручки: m → было | стало");
    for m in ["0.001", "0.01", "0.1", "1", "5"].map(d) {
        let new = amount(m, Decimal::ONE, Decimal::ONE, p, None, buy, buy);
        println!("{m} → {} | {}", old(m, p, None, buy, buy), new.round_dp(6));
        assert!(new <= m && new < p);
    }
    println!("Доля 0,8 при m = 0,001, доля выручки: спред → было | стало");
    for s in ["0.01", "0.25", "1.6", "10000"].map(d) {
        let sell = buy * (Decimal::ONE + s);
        let new = amount(
            d("0.001"),
            Decimal::ONE,
            Decimal::ONE,
            p,
            Some(share),
            buy,
            sell,
        );
        println!(
            "{s} → {} | {}",
            old(d("0.001"), p, Some(share), buy, sell),
            new.round_dp(6)
        );
        // Грязная прибыль на 1 USDT выручки — 1 − 1/(1 + спред); доля 0,8 оставляет 20 % её.
        let gross = Decimal::ONE - buy / sell;
        assert!(
            (new - share * gross).abs() <= tol(p),
            "{s}: {new} vs {}",
            share * gross
        );
    }
}
