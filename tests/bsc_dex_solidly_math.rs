//! Файл: tests/bsc_dex_solidly_math.rs
//! Что делает: математика свопа СТАБИЛЬНОГО пула Solidly (math/swap_solidly.rs + math/solidly_math.rs, решение 80 —
//! экзотическая математика ve(3,3)-форков) на инварианте `x³y + xy³ = k` официального контракта
//! velodrome-finance/contracts (HEAD b3065d8b6702b14b094f9f6046b752cc9f78c43b), contracts/Pool.sol. Без сети, без
//! моков. Та же математика покрывает стабильные пулы ve(3,3)-форков на BSC (Topaz v2-stable, THENA V1 stable).
//!
//! Откуда якоря (решение 23): в официальных тестах форка (test/Pool.t.sol) ЕСТЬ числовые векторы getAmountOut для
//! стабильного пула FRAX/USDC (разные decimals 18/6, комиссия 1/10000) — воспроизводим их бит-в-бит. Их резервы: пул
//! засевается дважды (deployPoolWithOwner для owner и owner2: по TOKEN_1 FRAX + USDC_1 USDC), затем тест доливает
//! ещё — на первой проверке резервы 3e6 USDC / 3e18 FRAX, на второй 4e6 / 4e18. Дополнительно — точные целые якоря,
//! сверенные НЕЗАВИСИМЫМ Python-портом того же инварианта из того же файла контракта (как для Curve/DODO), плюс
//! инварианты кривой (симметрия f, get_y на балансе, сходимость Ньютона, рост k с ликвидностью) и граничные случаи.
//! Охват фазы A: ТОЛЬКО стабильный инвариант; разные decimals поддержаны. Чтение/квотер/граф/семейства — следующая фаза.
//! Кросс-чек против живого квотера Topaz/THENA V1 на записи реального пула — следующая фаза.
//! Кто вызывает: cargo test. От чего зависит: arb::venues::bsc_dex::math::swap_solidly.

use alloy_primitives::U256;

use arb::venues::bsc_dex::math::swap_solidly::{f, get_amount_out, get_y, k};

/// 10^18.
fn e18() -> U256 {
    U256::from(10u8).pow(U256::from(18u8))
}
/// 10^6 (decimals USDC).
fn e6() -> U256 {
    U256::from(10u8).pow(U256::from(6u8))
}
/// n·10^18.
fn e(n: u64) -> U256 {
    U256::from(n) * e18()
}
/// Разобрать десятичную строку wei в U256.
fn w(s: &str) -> U256 {
    s.parse().unwrap()
}

/// Официальный якорь O1 (test/Pool.t.sol, mintAndBurnTokensForPoolFraxUsdc): стабильный пул FRAX/USDC, резервы
/// 3e6 USDC / 3e18 FRAX, комиссия 1/10000. getAmountOut(1e6 USDC) = 982117769725505988 (FRAX, 18 знаков).
#[test]
fn official_frax_usdc_3x() {
    let out = get_amount_out(
        e6(),
        U256::from(3u8) * e6(),
        e(3),
        e6(),
        e18(),
        U256::from(1u8),
    )
    .unwrap();
    assert_eq!(out, w("982117769725505988"));
}

/// Официальный якорь O2 (test/Pool.t.sol, mintAndBurnTokensForPoolFraxUsdcOwner2): тот же пул после долива,
/// резервы 4e6 USDC / 4e18 FRAX, комиссия 1/10000. getAmountOut(1e6 USDC) = 992220948146798746.
#[test]
fn official_frax_usdc_4x() {
    let out = get_amount_out(
        e6(),
        U256::from(4u8) * e6(),
        e(4),
        e6(),
        e18(),
        U256::from(1u8),
    )
    .unwrap();
    assert_eq!(out, w("992220948146798746"));
}

/// Якорь A3 (независимый Python-порт инварианта): пул равных 18-знаковых токенов, резервы по 1_000_000,
/// вход 10_000, комиссия 0,04% (fee=4/10000). Сверено с контрактным _getAmountOut.
#[test]
fn anchor_equal18_fee4() {
    let out = get_amount_out(
        e(10_000),
        e(1_000_000),
        e(1_000_000),
        e18(),
        e18(),
        U256::from(4u8),
    )
    .unwrap();
    assert_eq!(out, w("9995995008000205979841"));
}

/// Якорь A4: тот же пул равных 18-знаков БЕЗ комиссии (fee=0) — чистый инвариант.
#[test]
fn anchor_equal18_no_fee() {
    let out = get_amount_out(
        e(10_000),
        e(1_000_000),
        e(1_000_000),
        e18(),
        e18(),
        U256::ZERO,
    )
    .unwrap();
    assert_eq!(out, w("9999995000005018743068"));
}

/// Якорь A5: обратное направление FRAX→USDC на резервах 4e18 FRAX / 4e6 USDC, вход 1e18 FRAX, комиссия 1/10000 →
/// 992220 (USDC, 6 знаков). Это денормализованный в 6 знаков тот же результат, что и официальный якорь O2
/// (992220948146798746 FRAX за 1 USDC): кривая симметрична, сбалансированный пул отдаёт симметрично в обе стороны.
#[test]
fn anchor_reverse_direction_symmetry() {
    let out = get_amount_out(
        e(1),
        e(4),
        U256::from(4u8) * e6(),
        e18(),
        e6(),
        U256::from(1u8),
    )
    .unwrap();
    assert_eq!(out, U256::from(992_220u32));
}

/// Инвариант: стабильный `_k` симметричен — f(x,y) = f(y,x) (кривая `x³y+xy³` симметрична по x,y).
#[test]
fn invariant_f_symmetric() {
    assert_eq!(f(e(2), e(3)).unwrap(), f(e(3), e(2)).unwrap());
    // k с decimals тоже: равные decimals → k(x,y) = f(x,y).
    assert_eq!(k(e(2), e(3), e18(), e18()).unwrap(), f(e(2), e(3)).unwrap());
}

/// Инвариант: на сбалансированном пуле get_y(x0 = reserve, xy, reserve) = reserve — неизменный баланс исходного
/// токена оставляет встречный на месте (сошлось в ноль шагов Ньютона). Сверено независимым портом.
#[test]
fn invariant_get_y_on_balance() {
    let r = e(1);
    let xy = k(r, r, e18(), e18()).unwrap();
    assert_eq!(get_y(r, xy, r).unwrap(), r);
}

/// Инвариант: k стабильной кривой растёт при росте любого резерва (больше ликвидности — больше инвариант).
#[test]
fn invariant_k_grows_with_liquidity() {
    let small = k(e(1_000_000), e(1_000_000), e18(), e18()).unwrap();
    let bigger = k(e(1_000_000), e(1_100_000), e18(), e18()).unwrap();
    assert!(bigger > small);
}

/// Инвариант: в стейбл-пуле выход меньше входа (проскальзывание + комиссия), но близко к 1:1; Ньютон сходится (Some).
/// На почти сбалансированном пуле равных знаков dy < dx и dy > dx·999/1000.
#[test]
fn invariant_out_below_in_but_close() {
    let dx = e(1_000);
    let dy = get_amount_out(
        dx,
        e(1_000_000),
        e(1_000_000),
        e18(),
        e18(),
        U256::from(4u8),
    )
    .unwrap();
    assert!(dy < dx, "в стейбл-пуле выход меньше входа");
    assert!(
        dy > dx * U256::from(999u16) / U256::from(1000u16),
        "но близко к 1:1"
    );
}

/// Граничный случай: нулевые decimals токена → деление в нормализации откатилось бы на контракте → None (не паника).
#[test]
fn edge_zero_decimals_is_none() {
    assert!(get_amount_out(e6(), e(3), e(3), U256::ZERO, e18(), U256::from(1u8)).is_none());
    assert!(get_amount_out(e6(), e(3), e(3), e6(), U256::ZERO, U256::from(1u8)).is_none());
    assert!(k(e(1), e(1), U256::ZERO, e18()).is_none());
}

/// Граничный случай: переполнение инварианта на огромных резервах (f: x·x > 2^256−1) → None, а не паника.
#[test]
fn edge_overflow_is_none() {
    let huge = U256::MAX / U256::from(2u8);
    assert!(f(huge, huge).is_none());
}
