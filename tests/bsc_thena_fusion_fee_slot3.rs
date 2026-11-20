//! Файл: tests/bsc_thena_fusion_fee_slot3.rs
//! Что проверяет: решение 463 (T463-02) — комиссия THENA Fusion следующего блока бота (math/fee_algebra_v1.rs,
//! math/adaptive_fee.rs; данные — как читает pools/fusion_oracle.rs) на повторе записи своего узла со слотом 3
//! (tests/data/bsc_thena_fusion_2026-11-20_slot3: 60 пулов, блоки 134760116–134760122; volumePerLiquidityInBlock —
//! старшие 128 бит слота 3; точки оракула index − 1, index, index + 1, index + 2, 0; начало окна — oracle.starts
//! (getTimepoints(время блока + 1, [WINDOW + 1, WINDOW]) от имени пула — тот же вызов, что у бота) и, на + 3 с, —
//! by_time[+3].getTimepoints(…, [0, WINDOW])[1]). Без допусков, точно:
//! 1. fee_at = комиссия, которую вернул официальный Quoter THENA (второе слово ответа), на всех 4 временах (время
//!    блока, время последней точки оракула, + 1 с, + 3 с) — 60 × 8 × 4 = 1920 из 1920; volumePerLiquidityInBlock ≠ 0
//!    у 51 пула из 60 (ветка с объёмом блока проверена на них).
//! 2. next_block_fee на время блока (то, что держит PoolState::Algebra.fee) = большая из Quoter на блоке и + 1 с — 60.
//! 3. Без объёма блока (volumePerLiquidityInBlock = 0) fee_at = getFee оператора на + 0 / 1 / 3 с — 180 из 180.
//! 4. Обмен бота (math/swap_algebra, карта по сжатым словам) с комиссией fee_at = виртуальный квотер (вход и выход) и
//!    Quoter (выход) до wei: 1880 полных; 40 — неполный обмен у САМОГО пула (вход виртуального квотера меньше суммы:
//!    0x117e…e3, 0x2f6c…98, 0x52dd…15, 0x987c…06 — 1 → 0, 1000 б.п.; 0x58b3…09 — 0 → 1, все суммы; 0xbf5e…3b — 0 → 1,
//!    100 и 1000 б.п.), бот — тот же вход и выход с complete = false.
//!
//! Числа пунктов 1–3 — из прогона порта на записи (вне cargo, 20.11); пункт 4 — по записи (вход виртуального квотера).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/thena_fusion_common; arb::venues::bsc_dex::math::{adaptive_fee, fee_algebra_v1,
//! swap_algebra}.

mod thena_fusion_common;

use alloy_primitives::{I256, U256};
use serde_json::Value;

use arb::venues::bsc_dex::math::adaptive_fee::FeeConfig;
use arb::venues::bsc_dex::math::fee_algebra_v1::{
    Oracle, Timepoint, WindowStart, fee_at, next_block_fee,
};
use arb::venues::bsc_dex::math::swap_algebra;

use thena_fusion_common::{SLOT3, gs_fee, hex_u256, len, map_of, pools, signed, state, word};

/// Времена котировок записи и сдвиг от времени блока (None — время последней точки оракула).
const AT: [(&str, Option<u32>); 4] = [
    ("at_block", Some(0)),
    ("at_last_timepoint", None),
    ("at_plus_1", Some(1)),
    ("at_plus_3", Some(3)),
];

/// Точка оракула из ответа timepoints(uint256) (7 слов). Ошибок нет (тест падает).
fn timepoint(r: &Value) -> Timepoint {
    assert_eq!(len(r), 7);
    Timepoint {
        initialized: !word(r, 0).is_zero(),
        ts: word(r, 1).to::<u32>(),
        tick_cum: signed(r, 2).as_i64(),
        vol: word(r, 4).to::<u128>(),
        avg_tick: signed(r, 5).as_i32(),
        vpl: word(r, 6),
    }
}

/// feeConfig() оператора (9 слов). Ошибок нет (тест падает).
fn config(r: &Value) -> FeeConfig {
    let u = |i: usize| word(r, i).to::<u32>();
    FeeConfig {
        alpha1: u(0) as u16,
        alpha2: u(1) as u16,
        beta1: u(2),
        beta2: u(3),
        gamma1: u(4) as u16,
        gamma2: u(5) as u16,
        volume_beta: u(6),
        volume_gamma: u(7) as u16,
        base_fee: u(8) as u16,
    }
}

/// Значение `k` массива `j` ответа getTimepoints (4 массива: смещения, затем [длина, значения…]). Ошибок нет.
fn arr(r: &Value, j: usize, k: usize) -> U256 {
    let at = word(r, j).to::<usize>() / 32;
    assert!(word(r, at).to::<usize>() > k, "длина массива {j}");
    word(r, at + 1 + k)
}

/// Начало окна из ответа getTimepoints: значение `k`. Ошибок нет (тест падает).
fn start(r: &Value, k: usize) -> WindowStart {
    WindowStart {
        tick_cum: I256::from_raw(arr(r, 0, k)).as_i64(),
        vol: arr(r, 2, k).to::<u128>(),
        vpl: arr(r, 3, k),
    }
}

/// Оракул записи (`block_vpl` — volumePerLiquidityInBlock слота 3; false — 0). Возвращает: оракул, время блока, тик,
/// volumePerLiquidityInBlock. Ошибок нет (тест падает).
fn oracle(p: &Value, block_vpl: bool) -> (Oracle, u32, i32, u128) {
    let gs = &p["globalState()"]["result"];
    let (tick, index) = (signed(gs, 1).as_i32(), word(gs, 3).to::<u16>());
    let ts = hex_u256(&p["block"]["timestamp"]).to::<u32>();
    let around = |i: u16| timepoint(&p["timepoints_around"][i.to_string()]["result"]);
    let s = &p["oracle"]["starts"];
    assert_eq!(s["time"].as_u64(), Some(u64::from(ts) + 1));
    let g = &s["getTimepoints"]["result"];
    let plus3 = &p["oracle"]["by_time"][(ts + 3).to_string()]["getTimepoints"]["result"];
    let starts = vec![(ts, start(g, 0)), (ts + 1, start(g, 1)), (ts + 3, start(plus3, 1))];
    let slot = hex_u256(&p["storage_slot3"]["result"]);
    let vpl = (slot >> 128usize).to::<u128>();
    let o = Oracle {
        cfg: config(&p["oracle"]["feeConfig"]["result"]),
        index,
        prev: around(index.wrapping_sub(1)),
        last: timepoint(&p["timepoints"]["result"]),
        next1: around(index.wrapping_add(1)),
        next2: around(index.wrapping_add(2)),
        zero: around(0),
        vpl_in_block: if block_vpl { vpl } else { 0 },
        fee: gs_fee(p),
        starts,
    };
    (o, ts, tick, vpl)
}

/// Время котировки `at`: время блока + сдвиг или время последней точки оракула. Ошибок нет.
fn time_of(o: &Oracle, ts: u32, d: Option<u32>) -> u32 {
    d.map_or(o.last.ts, |d| ts + d)
}

#[test]
fn fee_equals_quoter_on_all_times() {
    let (mut cases, mut busy) = (0, 0);
    for p in pools(SLOT3, 60) {
        let (o, ts, tick, vpl) = oracle(&p, true);
        busy += usize::from(vpl != 0);
        for q in p["quotes"].as_array().unwrap() {
            for (at, d) in AT {
                let qf = word(&q[at]["quoter"]["result"], 1).to::<u32>();
                let t = time_of(&o, ts, d);
                assert_eq!(fee_at(&o, t, tick), Some(qf), "{} {at}", p["pool"]);
                cases += 1;
            }
        }
    }
    assert_eq!((cases, busy), (1920, 51));
}

#[test]
fn next_block_fee_is_max_of_block_and_plus_1() {
    for p in pools(SLOT3, 60) {
        let (o, ts, tick, _) = oracle(&p, true);
        let q = &p["quotes"][0];
        let f = |at: &str| word(&q[at]["quoter"]["result"], 1).to::<u32>();
        let want = f("at_block").max(f("at_plus_1"));
        assert_eq!(next_block_fee(&o, ts, tick), Some(want), "{}", p["pool"]);
    }
}

#[test]
fn fee_without_block_volume_equals_get_fee() {
    let mut cases = 0;
    for p in pools(SLOT3, 60) {
        let (o, ts, tick, _) = oracle(&p, false);
        for d in [0u32, 1, 3] {
            let b = &p["oracle"]["by_time"][(ts + d).to_string()];
            let gf = word(&b["getFee"]["result"], 0).to::<u32>();
            assert_eq!(fee_at(&o, ts + d, tick), Some(gf), "{} +{d} с", p["pool"]);
            cases += 1;
        }
    }
    assert_eq!(cases, 180);
}

#[test]
fn swap_with_bot_fee_equals_quoter() {
    let (mut exact, mut partial) = (0, 0);
    for p in pools(SLOT3, 60) {
        let (o, ts, tick, _) = oracle(&p, true);
        let m = map_of(&p);
        for q in p["quotes"].as_array().unwrap() {
            let zfo = q["zero_for_one"].as_bool().unwrap();
            let amount = hex_u256(&q["amount_in"]);
            for (at, d) in AT {
                let (qr, vr) = (&q[at]["quoter"]["result"], &q[at]["virtual"]["result"]);
                let (out, v_in, v_out) = (word(qr, 0), word(vr, 0), word(vr, 1));
                let case = format!("{} {at} zfo={zfo} bps={}", p["pool"], q["bps"]);
                assert_eq!(v_out, out, "{case}: виртуальный = Quoter");
                let fee = fee_at(&o, time_of(&o, ts, d), tick).expect("комиссия");
                let r = swap_algebra::swap(&state(&p, &m, fee), zfo, true, amount).expect("расчёт");
                assert_eq!((r.amount_in, r.amount_out), (v_in, v_out), "{case}");
                if r.complete {
                    exact += 1;
                } else {
                    // Неполный у самого пула (вход квотера меньше суммы) — бот = пул до wei, без завышения.
                    assert!(v_in < amount, "{case}: пул взял всю сумму, а бот — нет");
                    partial += 1;
                }
            }
        }
    }
    assert_eq!((exact, partial), (1880, 40));
}
