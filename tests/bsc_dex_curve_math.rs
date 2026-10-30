//! Файл: tests/bsc_dex_curve_math.rs
//! Что делает: математика свопа Curve StableSwap (math/swap_curve.rs + math/curve_math.rs, решение 80 — экзотическая
//! математика Curve/Ellipsis stableswap) на инварианте официального контракта curvefi/curve-contract
//! (HEAD 574f44027d089de0eac765f5a74ea5ae96aba968), contracts/pool-templates/base/SwapTemplateBase.vy. Без сети, без
//! моков. Та же математика покрывает Ellipsis на BSC (форк Curve — инвариант тот же; репо Ellipsis приватное, поэтому
//! эталон берётся у официального публичного Curve).
//!
//! Откуда якоря (решение 23): официальные тесты Curve — property/fuzz (brownie + hypothesis), фиксированных числовых
//! векторов get_dy в них нет. Поэтому, как для DODO (86a) и WooFi: зафиксированы точные целые якоря, сверенные
//! НЕЗАВИСИМЫМ Python-портом инварианта (_get_D/_get_y/get_dy того же файла контракта), плюс проверены известные
//! инварианты StableSwap (сходимость Ньютона, D=Σx на сбалансированном пуле, симметрия get_y, монотонность D).
//! Охват фазы A: PLAIN StableSwap с РАВНЫМИ rate-множителями (RATES[k]=PRECISION, все монеты 18 знаков) → xp=balances;
//! amp = A·A_PRECISION (A·100). Meta/разные decimals/квотер/граф — фаза B.
//! Кросс-чек против живого пула Curve/Ellipsis на записи — фаза B (семейство).
//! Кто вызывает: cargo test. От чего зависит: arb::venues::bsc_dex::math::swap_curve.

use alloy_primitives::U256;

use arb::venues::bsc_dex::math::swap_curve::{get_d, get_dy, get_y};

/// 10^18.
fn e18() -> U256 {
    U256::from(10u8).pow(U256::from(18u8))
}
/// n·10^18.
fn e(n: u64) -> U256 {
    U256::from(n) * e18()
}
/// Разобрать десятичную строку wei в U256.
fn w(s: &str) -> U256 {
    s.parse().unwrap()
}
/// A с префактором A_PRECISION=100 (как self._A() контракта).
fn amp(a: u64) -> U256 {
    U256::from(a) * U256::from(100u8)
}

/// Якорь A1 (независимый Python-порт инварианта): 2-монетный сбалансированный плоский пул, A=100, баланс по 1_000_000.
/// D = Σx на балансе; get_dy(0→1, 100) с комиссией 0,04% (fee=4_000_000 / 1e10).
#[test]
fn anchor_2coin_balanced() {
    let bal = [e(1_000_000), e(1_000_000)];
    let a = amp(100);

    // D сбалансированного пула = сумма балансов (точный инвариант StableSwap).
    let d = get_d(&bal, a).unwrap();
    assert_eq!(d, e(2_000_000));

    let dy = get_dy(0, 1, e(100), &bal, a, U256::from(4_000_000u32)).unwrap();
    assert_eq!(dy, w("99959901029799990394"));
}

/// Якорь A2: 3-монетный НЕсбалансированный плоский пул, A=200, балансы 1_000_000 / 1_500_000 / 800_000.
/// get_dy(0→2, 50_000) с комиссией 0,01% (fee=1_000_000).
#[test]
fn anchor_3coin_unbalanced() {
    let bal = [e(1_000_000), e(1_500_000), e(800_000)];
    let a = amp(200);

    let d = get_d(&bal, a).unwrap();
    assert_eq!(d, w("3299403001609667528567716"));

    let dy = get_dy(0, 2, e(50_000), &bal, a, U256::from(1_000_000u32)).unwrap();
    assert_eq!(dy, w("49899039960214289913850"));
}

/// Якорь A3: тот же 3-монетный пул, обратное направление (2→0), БЕЗ комиссии (fee=0) — чистый инвариант.
#[test]
fn anchor_3coin_reverse_no_fee() {
    let bal = [e(1_000_000), e(1_500_000), e(800_000)];
    let a = amp(200);
    let dy = get_dy(2, 0, e(50_000), &bal, a, U256::ZERO).unwrap();
    assert_eq!(dy, w("50056261536919020415534"));
}

/// Якорь A4: мелкий 2-монетный пул, A=50, балансы 10_000 / 12_000; get_dy(0→1, 1_000), комиссия 0,04%.
#[test]
fn anchor_2coin_small() {
    let bal = [e(10_000), e(12_000)];
    let a = amp(50);

    let d = get_d(&bal, a).unwrap();
    assert_eq!(d, w("21998203050455157567527"));

    let dy = get_dy(0, 1, e(1_000), &bal, a, U256::from(4_000_000u32)).unwrap();
    assert_eq!(dy, w("1001396229326524242567"));
}

/// Инвариант: на сбалансированном 2-монетном пуле get_y(i=0, j=1, x = balances[0]) = balances[1] —
/// неизменный баланс i оставляет j на месте (симметрия кривой). Сверено независимым портом.
#[test]
fn invariant_get_y_symmetry() {
    let bal = [e(1_000_000), e(1_000_000)];
    let a = amp(100);
    let y = get_y(0, 1, bal[0], &bal, a).unwrap();
    assert_eq!(y, e(1_000_000));
}

/// Инвариант: D монотонно растёт при росте любого баланса (больше ликвидности — больше D).
#[test]
fn invariant_d_monotonic() {
    let a = amp(100);
    let small = [e(1_000_000), e(1_000_000)];
    let bigger = [e(1_000_000), e(1_100_000)];
    assert!(get_d(&bigger, a).unwrap() > get_d(&small, a).unwrap());
}

/// Инвариант: своп в стейбл-пуле отдаёт меньше, чем вложено (проскальзывание + комиссия), но близко к 1:1;
/// Ньютон сходится (Some). На почти сбалансированном пуле dy < dx и dy > dx·999/1000.
#[test]
fn invariant_dy_below_dx_but_close() {
    let bal = [e(1_000_000), e(1_000_000)];
    let a = amp(1000);
    let dx = e(1_000);
    let dy = get_dy(0, 1, dx, &bal, a, U256::from(4_000_000u32)).unwrap();
    assert!(dy < dx, "в стейбл-пуле выход меньше входа");
    assert!(
        dy > dx * U256::from(999u16) / U256::from(1000u16),
        "но близко к 1:1"
    );
}

/// Граничный случай: нулевой баланс монеты → `_x·N = 0` в get_d, контракт откатился бы → None (не паника).
#[test]
fn edge_zero_balance_is_none() {
    let bal = [e(1_000_000), U256::ZERO];
    let a = amp(100);
    assert!(get_d(&bal, a).is_none());
    assert!(get_dy(0, 1, e(100), &bal, a, U256::ZERO).is_none());
}

/// Граничный случай: неверные индексы (i==j или вне диапазона) → None.
#[test]
fn edge_bad_indices_is_none() {
    let bal = [e(1_000_000), e(1_000_000)];
    let a = amp(100);
    assert!(get_dy(0, 0, e(100), &bal, a, U256::ZERO).is_none());
    assert!(get_dy(0, 5, e(100), &bal, a, U256::ZERO).is_none());
    assert!(get_y(1, 1, bal[0], &bal, a).is_none());
}
