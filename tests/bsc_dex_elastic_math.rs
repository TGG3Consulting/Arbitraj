//! Файл: tests/bsc_dex_elastic_math.rs
//! Что делает: математика обмена KyberSwap Elastic (math/swap_elastic.rs, решение 81) на ОФИЦИАЛЬНЫХ значениях
//! контракта KyberNetwork/ks-elastic-sc (HEAD 4ab08c0). Без сети. Шаг (computeSwapStep и его части) сверяется с
//! логами официального теста test/libraries/SwapMath.spec.ts (liquidity=1e18, fee=300, цены encodePriceSqrt):
//! calcReachAmount exactInput = 4995092120267736 (0.004995092120267736), estimateIncrementalLiquidity =
//! 7492638180401 (0.000007492638180401), encodePriceSqrt(101,100)=79623317895830914510639640423 — совпадают точно.
//! Клэмпинг computeSwapStep (used→specified / used→reach) — точные assert'ы спеки. exactOutput проверяется ВНУТРЕННЕЙ
//! согласованностью с exactInput (одно и то же движение цены). Цикл свопа (MAX_TICK_DISTANCE=480, рост reinvestL,
//! пересечение тика baseL±=liquidityNet) — на эталоне, посчитанном СТРОГО по формулам контракта (Python-порт Pool.swap).
//! ВАЖНО: комментарии exactOutput в SwapMath.spec.ts (-0.004970250488226176) УСТАРЕЛИ относительно текущего кода
//! контракта — текущая формула (и мой порт) дают значение, согласованное с exactInput (см. exact_output_consistency).
//! Кросс-чек против живого Elastic QuoterV2 на записи реального пула — фаза 81b.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::math::{swap_elastic, tick_math}, pools::ticks (TickMap/TickData).

use std::collections::BTreeMap;

use alloy_primitives::{I256, U256};

use arb::venues::bsc_dex::math::swap_elastic::{
    self, ElasticState, Swapped, calc_reach_amount, calc_returned_amount, compute_swap_step,
    estimate_incremental_liquidity,
};
use arb::venues::bsc_dex::math::tick_math::sqrt_ratio_at_tick;
use arb::venues::bsc_dex::pools::ticks::{TickData, TickMap};

/// liquidity = 1e18, fee = 300 feeUnits (0.03%), как в SwapMath.spec.ts.
fn l() -> U256 {
    U256::from(1_000_000_000_000_000_000u64)
}
const FEE: u32 = 300;
/// encodePriceSqrt(1,1) = 2^96.
fn p_start() -> U256 {
    sqrt_ratio_at_tick(0).unwrap()
}
/// encodePriceSqrt(100,101) = floor(sqrt(100/101)·2^96) — цена вниз (token0→token1).
fn p_down() -> U256 {
    "78834968213693974763009544974".parse().unwrap()
}
/// encodePriceSqrt(101,100) — цена вверх; значение из комментария SwapMath.spec.ts (совпадает точно).
fn p_up() -> U256 {
    "79623317895830914510639640423".parse().unwrap()
}
fn i(v: i128) -> I256 {
    I256::try_from(v).unwrap()
}

/// calcReachAmount exactInput — официальное логированное значение 4995092120267736 (обе стороны).
#[test]
fn calc_reach_exact_input_official() {
    // token0 → token1 (isToken0=true), цена вниз.
    assert_eq!(
        calc_reach_amount(l(), p_start(), p_down(), FEE, true, true).unwrap(),
        i(4_995_092_120_267_736)
    );
    // token1 → token0 (isToken0=false), цена вверх.
    assert_eq!(
        calc_reach_amount(l(), p_start(), p_up(), FEE, true, false).unwrap(),
        i(4_995_092_120_267_736)
    );
}

/// estimateIncrementalLiquidity exactInput — официальное логированное 7492638180401 (= used·fee/2).
#[test]
fn estimate_incremental_official() {
    let used = U256::from(4_995_092_120_267_736u64) - U256::from(1u8); // usedAmount.sub(ONE) из спеки
    assert_eq!(
        estimate_incremental_liquidity(used, l(), p_start(), FEE, true, true).unwrap(),
        U256::from(7_492_638_180_401u64)
    );
    assert_eq!(
        estimate_incremental_liquidity(used, l(), p_start(), FEE, true, false).unwrap(),
        U256::from(7_492_638_180_401u64)
    );
}

/// computeSwapStep: specifiedAmount < reach → used = specified; specifiedAmount > reach → used = reach (точные
/// assert'ы SwapMath.spec.ts «#computeSwapStep - new branch»); при достижении цели finalPrice ≥ priceEnd.
#[test]
fn compute_step_clamp_official() {
    let reach = 4_995_092_120_267_736i128;
    // меньше reach — обрезается до specified.
    let s1 = compute_swap_step(l(), p_start(), p_down(), FEE, i(reach - 10), true, true).unwrap();
    assert_eq!(s1.used, i(reach - 10));
    // больше reach — доходит до цели, used = reach, finalPrice ≥ priceEnd (цена вниз: next_sqrt ≥ p_down).
    let s2 = compute_swap_step(l(), p_start(), p_down(), FEE, i(reach + 10), true, true).unwrap();
    assert_eq!(s2.used, i(reach));
    assert!(s2.next_sqrt >= p_down(), "finalPrice ≥ priceEnd (спека)");
}

/// exactOutput согласован с exactInput: token1, нужный чтобы ДОЙТИ до p_down (calcReachAmount exactOutput !isToken0),
/// равен token1, который ВЫХОДИТ при exactInput token0 до того же p_down (calcReturnedAmount). Отрицателен (выход/нужно
/// отдать меньше). Этим проверяем exactOutput-ветку без опоры на устаревший комментарий спеки.
#[test]
fn exact_output_consistency() {
    let used_a = U256::from(4_995_092_120_267_736u64) - U256::from(1u8);
    let lc = estimate_incremental_liquidity(used_a, l(), p_start(), FEE, true, true).unwrap();
    let out_token1 = calc_returned_amount(l(), p_start(), p_down(), lc, true, true).unwrap();
    assert!(
        out_token1.is_negative(),
        "выход token1 отрицателен (его отдаём)"
    );
    let reach_out = calc_reach_amount(l(), p_start(), p_down(), FEE, false, false).unwrap();
    assert_eq!(
        reach_out, out_token1,
        "exactOutput token1-до-цели = exactInput token1-выход (согласованность; комментарий спеки устарел)"
    );
}

/// Карта тиков: один инициализированный тик `tick` с liquidityNet `net`, прочитанное окно `words`.
fn ticks(tick: i32, net: i128, words: (i16, i16)) -> TickMap {
    let mut m = BTreeMap::new();
    m.insert(
        tick,
        TickData {
            gross: net.unsigned_abs(),
            net,
        },
    );
    TickMap {
        spacing: 1,
        ticks: m,
        words,
    }
}

/// Цикл свопа с пересечением одного тика — эталон строго по формулам контракта (Python-порт Pool.swap, те же
/// 480-ограничение и рост reinvestL). exactInput token1 (цена вверх), reinvestL = 0.
#[test]
fn swap_loop_crosses_tick() {
    // Малый: тик +10 (net +2e17), вход 5e16 — переходит тик и завершается.
    let t = ticks(10, 200_000_000_000_000_000, (-50, 50));
    let s = ElasticState {
        sqrt_p: p_start(),
        current_tick: 0,
        base_l: 1_000_000_000_000_000_000,
        reinvest_l: 0,
        fee_units: FEE,
        ticks: &t,
    };
    assert_eq!(
        swap_elastic::swap(&s, false, true, U256::from(50_000_000_000_000_000u64)).unwrap(),
        Swapped {
            amount_in: U256::from(50_000_000_000_000_000u64),
            amount_out: U256::from(47_849_664_482_374_746u64),
            complete: true,
        }
    );
    // Большой: тик +60 (net +5e17), вход 2e18 — переход + множество 480-шагов (reinvestL растёт) до исчерпания.
    let t2 = ticks(60, 500_000_000_000_000_000, (-500, 500));
    let s2 = ElasticState {
        sqrt_p: p_start(),
        current_tick: 0,
        base_l: 1_000_000_000_000_000_000,
        reinvest_l: 0,
        fee_units: FEE,
        ticks: &t2,
    };
    assert_eq!(
        swap_elastic::swap(&s2, false, true, U256::from(2_000_000_000_000_000_000u64)).unwrap(),
        Swapped {
            amount_in: U256::from(2_000_000_000_000_000_000u64),
            amount_out: U256::from(854_266_939_761_588_083u64),
            complete: true,
        }
    );
}

/// Узкое окно тиков: цена выходит за прочитанное — итог частичный (complete = false), суммы — исполненная часть
/// (эталон по формулам контракта).
#[test]
fn swap_loop_incomplete_outside_window() {
    let t = ticks(60, 500_000_000_000_000_000, (-2, 2)); // слова (-2,2): тики до 2·256+255 = 767
    let s = ElasticState {
        sqrt_p: p_start(),
        current_tick: 0,
        base_l: 1_000_000_000_000_000_000,
        reinvest_l: 0,
        fee_units: FEE,
        ticks: &t,
    };
    let out =
        swap_elastic::swap(&s, false, true, U256::from(2_000_000_000_000_000_000u64)).unwrap();
    assert!(
        !out.complete,
        "цена вышла за прочитанное окно — частичный итог"
    );
    assert_eq!(out.amount_in, U256::from(57_225_404_597_530_393u64));
    assert_eq!(out.amount_out, U256::from(54_853_706_175_833_300u64));
}
