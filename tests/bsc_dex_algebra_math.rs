//! Файл: tests/bsc_dex_algebra_math.rs
//! Что делает: математика обмена Algebra (THENA Fusion и форки Algebra V1/V1.9; math/swap_algebra.rs, решения 80, 81)
//! на ОФИЦИАЛЬНЫХ числах контракта cryptoalgebra/AlgebraV1.9. Без сети. Две части.
//! Часть 1 — шаг цены: доказывает, что наш math/swap_step::step_v3 (порт Uniswap V3 SwapMath) до последнего целого
//! воспроизводит PriceMovementMath.movePriceTowardsTarget Algebra — на всех девяти векторах официального теста
//! test/PriceMovement.spec.ts (обёртка PriceMovementMathTest: zeroToOne = target < price; amountRemaining ≥ 0 — точный
//! вход, < 0 — точный выход; комиссия в единицах 1e-6). Значения encodePriceSqrt совпадают с математикой биржи.
//! Поэтому отдельного модуля шага Algebra не нужно (см. шапку math/swap_algebra.rs).
//! Часть 2 — цикл свопа (swap/swap_to): поиск следующего тика по карте, пересечение инициализированного тика
//! (liquidity ± liquidityDelta), остановка на пределе цены и частичный итог на краю прочитанного окна — сверяются
//! СТЫКОВКОЙ тех же step_v3, что доказаны в части 1 (эталон — не мок, а те же проверенные формулы).
//! Кросс-чек против живого Algebra QuoterV2 на записи реального пула THENA — фаза B (как у Elastic 81b).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::math::{swap_algebra, swap_step, tick_math}, pools::ticks (TickMap/TickData).

use std::collections::BTreeMap;

use alloy_primitives::{I256, U256};

use arb::venues::bsc_dex::math::swap_algebra::{self, AlgebraState};
use arb::venues::bsc_dex::math::swap_step::step_v3;
use arb::venues::bsc_dex::math::tick_math::{MAX_TICK, max_sqrt_ratio, sqrt_ratio_at_tick};
use arb::venues::bsc_dex::pools::ticks::{TickData, TickMap};

fn u(s: &str) -> U256 {
    s.parse().unwrap()
}

/// encodePriceSqrt(1,1) = 2^96.
fn p11() -> U256 {
    u("79228162514264337593543950336")
}

/// Один вызов step_v3 как PriceMovementMath.movePriceTowardsTarget (знак суммы: вход ≥ 0, выход < 0).
/// Возвращает: (sqrtQ, amountIn, amountOut, feeAmount).
fn step(
    sqrt: U256,
    target: U256,
    liq: u128,
    amount: &str,
    exact_in: bool,
    fee: u32,
) -> (U256, U256, U256, U256) {
    let a = I256::try_from(u(amount)).unwrap();
    let a = if exact_in { a } else { -a };
    step_v3(sqrt, target, liq, a, fee).unwrap()
}

/// Часть 1: девять официальных векторов PriceMovement.spec.ts — step_v3 совпадает с Algebra до последнего целого.
#[test]
fn step_matches_official_algebra_vectors() {
    let l2 = 2_000_000_000_000_000_000u128; // expandTo18Decimals(2)
    let target_101_100 = u("79623317895830914510639640423"); // encodePriceSqrt(101,100)

    // 1) exact amount in that gets capped at price target in one for zero.
    let (q, ai, ao, fa) = step(p11(), target_101_100, l2, "1000000000000000000", true, 600);
    assert_eq!(ai, u("9975124224178055"));
    assert_eq!(fa, u("5988667735148"));
    assert_eq!(ao, u("9925619580021728"));
    assert_eq!(q, target_101_100, "price capped at target");

    // 2) exact amount out that gets capped at price target in one for zero.
    let (q, ai, ao, fa) = step(p11(), target_101_100, l2, "1000000000000000000", false, 600);
    assert_eq!(ai, u("9975124224178055"));
    assert_eq!(fa, u("5988667735148"));
    assert_eq!(ao, u("9925619580021728"));
    assert_eq!(q, target_101_100);

    // 3) exact amount in that is fully spent in one for zero (target encodePriceSqrt(1000,100), not reached).
    let t = u("250541448375047931186413801569");
    let (q, ai, ao, fa) = step(p11(), t, l2, "1000000000000000000", true, 600);
    assert_eq!(ai, u("999400000000000000"));
    assert_eq!(fa, u("600000000000000"));
    assert_eq!(ao, u("666399946655997866"));
    assert!(q < t, "price does not reach target");

    // 4) exact amount out that is fully received in one for zero (target encodePriceSqrt(10000,100), not reached).
    let t = u("792281625142643375935439503360");
    let (q, ai, ao, fa) = step(p11(), t, l2, "1000000000000000000", false, 600);
    assert_eq!(ai, u("2000000000000000000"));
    assert_eq!(fa, u("1200720432259356"));
    assert_eq!(ao, u("1000000000000000000"));
    assert!(q < t);

    // 5) amount out is capped at the desired amount out.
    let (q, ai, ao, fa) = step(
        u("417332158212080721273783715441582"),
        u("1452870262520218020823638996"),
        159_344_665_391_607_089_467_575_320_103_u128,
        "1",
        false,
        1,
    );
    assert_eq!(ai, U256::from(1u8));
    assert_eq!(fa, U256::from(1u8));
    assert_eq!(ao, U256::from(1u8));
    assert_eq!(q, u("417332158212080721273783715441581"));

    // 6) target price of 1 uses partial input amount.
    let (q, ai, ao, fa) = step(
        U256::from(2u8),
        U256::from(1u8),
        1u128,
        "3915081100057732413702495386755767",
        true,
        1,
    );
    assert_eq!(ai, u("39614081257132168796771975168"));
    assert_eq!(fa, u("39614120871253040049813"));
    assert_eq!(ao, U256::ZERO);
    assert_eq!(q, U256::from(1u8));

    // 7) entire input amount taken as fee.
    let (q, ai, ao, fa) = step(
        U256::from(2413u16),
        u("79887613182836312"),
        1_985_041_575_832_132_834_610_021_537_970_u128,
        "10",
        true,
        1872,
    );
    assert_eq!(ai, U256::ZERO);
    assert_eq!(fa, U256::from(10u8));
    assert_eq!(ao, U256::ZERO);
    assert_eq!(q, U256::from(2413u16));

    // 8) intermediate insufficient liquidity, exact output, price up (target = sqrtP*11/10).
    let (q, ai, ao, fa) = step(
        u("20282409603651670423947251286016"),
        u("22310650564016837466341976414617"),
        1024u128,
        "4",
        false,
        3000,
    );
    assert_eq!(ao, U256::ZERO);
    assert_eq!(q, u("22310650564016837466341976414617"));
    assert_eq!(ai, U256::from(26215u32));
    assert_eq!(fa, U256::from(79u8));

    // 9) intermediate insufficient liquidity, exact output, price down (target = sqrtP*9/10).
    let (q, ai, ao, fa) = step(
        u("20282409603651670423947251286016"),
        u("18254168643286503381552526157414"),
        1024u128,
        "263000",
        false,
        3000,
    );
    assert_eq!(ao, U256::from(26214u32));
    assert_eq!(q, u("18254168643286503381552526157414"));
    assert_eq!(ai, U256::from(1u8));
    assert_eq!(fa, U256::from(1u8));
}

/// Карта тиков с одним инициализированным тиком (net) и широким окном (всё прочитано).
fn one_tick(tick: i32, net: i128) -> TickMap {
    let mut ticks = BTreeMap::new();
    ticks.insert(
        tick,
        TickData {
            gross: net.unsigned_abs(),
            net,
        },
    );
    TickMap {
        spacing: 60,
        ticks,
        words: (i16::MIN, i16::MAX),
    }
}

/// Часть 2: цикл свопа пересекает один инициализированный тик — итог равен стыковке двух step_v3
/// (до тика с L0, после тика с L0+net). Направление oneForZero (цена вверх, exactIn).
#[test]
fn swap_crosses_one_tick() {
    let l0 = 2_000_000_000_000_000_000u128;
    let net = 1_000_000_000_000_000_000i128;
    let tm = one_tick(60, net);
    let s = AlgebraState {
        sqrt_price: p11(),
        tick: 0,
        liquidity: l0,
        fee: 600,
        ticks: &tm,
    };
    let amount = u("1000000000000000000"); // достаточно, чтобы пересечь тик 60 и продолжить
    let got = swap_algebra::swap(&s, false, true, amount).unwrap();

    // Эталон: шаг 1 до цены тика 60 (капает), пересечение (+net), шаг 2 остатком до предела цены.
    let sqrt60 = sqrt_ratio_at_tick(60).unwrap();
    let a1 = I256::try_from(amount).unwrap();
    let (q1, in1, out1, fe1) = step_v3(p11(), sqrt60, l0, a1, 600).unwrap();
    assert_eq!(
        q1, sqrt60,
        "шаг 1 должен дойти до тика 60 (иначе тест не пересекает тик)"
    );
    let spent1 = in1 + fe1;
    let rem = amount - spent1;
    let l1 = l0 + net.unsigned_abs(); // oneForZero прибавляет liquidityDelta
    let limit = max_sqrt_ratio() - U256::from(1u8);
    let target2 = sqrt_ratio_at_tick(MAX_TICK).unwrap().min(limit);
    let (q2, in2, out2, fe2) =
        step_v3(sqrt60, target2, l1, I256::try_from(rem).unwrap(), 600).unwrap();
    assert!(
        q2 < target2,
        "остаток должен исполниться до предела (частичного не ждём)"
    );

    assert!(got.complete);
    assert_eq!(got.amount_in, spent1 + in2 + fe2);
    assert_eq!(got.amount_out, out1 + out2);
}

/// Цикл свопа останавливается на пределе цены (swap_to), не пересекая тик за пределом.
#[test]
fn swap_stops_at_price_limit() {
    let l0 = 2_000_000_000_000_000_000u128;
    let tm = one_tick(60, 1_000_000_000_000_000_000i128);
    let s = AlgebraState {
        sqrt_price: p11(),
        tick: 0,
        liquidity: l0,
        fee: 600,
        ticks: &tm,
    };
    let limit = sqrt_ratio_at_tick(30).unwrap(); // предел до тика 60 — тик не пересекается
    let amount = u("1000000000000000000");
    let (got, reached) = swap_algebra::swap_to(&s, false, true, amount, limit).unwrap();
    assert!(reached, "цена должна дойти ровно до предела");

    let a1 = I256::try_from(amount).unwrap();
    let (q, inp, out, fe) = step_v3(p11(), limit, l0, a1, 600).unwrap();
    assert_eq!(q, limit);
    assert_eq!(got.amount_in, inp + fe);
    assert_eq!(got.amount_out, out);
    assert!(!got.complete, "сумма не исчерпана — упёрлись в предел");
}

/// На краю прочитанного окна (за ним тики неизвестны) итог частичный (complete = false).
#[test]
fn swap_partial_at_window_edge() {
    let tm = TickMap {
        spacing: 60,
        ticks: BTreeMap::new(),
        words: (0, 0), // окно сырых тиков [0, 255]
    };
    let s = AlgebraState {
        sqrt_price: p11(),
        tick: 0,
        liquidity: 2_000_000_000_000_000_000u128,
        fee: 600,
        ticks: &tm,
    };
    // Большая сумма вверх: цена дойдёт до края окна (тик 255), дальше тики неизвестны.
    let got = swap_algebra::swap(&s, false, true, u("1000000000000000000")).unwrap();
    assert!(!got.complete, "за краем окна тиков итог частичный");
    assert!(got.amount_out > U256::ZERO);
}
