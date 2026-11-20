//! Файл: tests/bsc_thena_fusion_fee_t237.rs
//! Что проверяет: решение 463 (T463-02) — порт комиссии следующего блока THENA Fusion (math/adaptive_fee.rs,
//! math/fee_algebra_v1.rs) на записи tests/data/bsc_thena_fusion_2026-11-20_t237 (60 пулов; данные оракула: feeConfig
//! оператора, timepoints(index − 2, − 1, index, + 1), от имени пула getFee / getAverages / getTimepoints(T, [0, WINDOW])
//! на T = время блока + 0, 1, 3 с; Quoter на те же времена).
//! 1. AdaptiveFee: get_fee(getAverages.volatility / 15, getAverages.volumePerLiquidity, feeConfig) = getFee оператора —
//!    60 × 3 = 180 из 180.
//! 2. fee_at (новая точка оракула, средние после записи, AdaptiveFee) при volumePerLiquidityInBlock = 0 = getFee
//!    оператора (тот же расчёт без записи точки) — 180 из 180. Начало окна — getTimepoints(T, [0, WINDOW])[1] записи
//!    (тот же вызов делает бот, pools/fusion_fee.rs). Точек index + 2 и 0 в записи нет: getTimepoints(T, [.., WINDOW])
//!    не откатился ни у одного пула — старейшая точка не моложе T − WINDOW, и её накопители в эту ветку расчёта не
//!    входят; поэтому вместо неё — точка «инициализирована, время 0» (то же сравнение lteConsideringOverflow).
//! 3. Своп (Quoter) на T: комиссия не ниже getFee оператора у всех 180; у 27 пулов (81 случай) — выше: своп пишет
//!    точку с volumePerLiquidityInBlock пула (слот 3, в записи его нет — нужен повтор записи T237-02); у 33 пулов
//!    (99 случаев) — равна. Пример 0x1b93…2210: getFee 1542 = Quoter 1542 на + 0 / 1 / 3 с, globalState.fee 1537.
//! Кто вызывает: cargo test.
//! От чего зависит: tests/thena_fusion_common; arb::venues::bsc_dex::math::{adaptive_fee, fee_algebra_v1}.

mod thena_fusion_common;

use alloy_primitives::U256;
use serde_json::Value;

use arb::venues::bsc_dex::math::adaptive_fee::{FeeConfig, get_fee};
use arb::venues::bsc_dex::math::fee_algebra_v1::{Oracle, Timepoint, WindowStart, fee_at};

use thena_fusion_common::{T237, gs_fee, hex_u256, len, pools, signed, word};

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

/// Точка записи `timepoints_around[i]`; нет её в записи — «инициализирована, время 0» (см. шапку, пункт 2).
fn around(p: &Value, i: u16) -> Timepoint {
    match p["timepoints_around"].get(i.to_string()) {
        Some(v) => timepoint(&v["result"]),
        None => Timepoint {
            initialized: true,
            ..Timepoint::default()
        },
    }
}

/// Оракул записи с volumePerLiquidityInBlock = 0 и началом окна по getTimepoints[1] на каждом T. Возвращает: оракул,
/// время блока, тик. Ошибок нет (тест падает).
fn oracle(p: &Value) -> (Oracle, u32, i32) {
    let gs = &p["globalState()"]["result"];
    let (tick, index) = (signed(gs, 1).as_i32(), word(gs, 3).to::<u16>());
    let ts = hex_u256(&p["block"]["timestamp"]).to::<u32>();
    let by = &p["oracle"]["by_time"];
    let mut starts = Vec::new();
    for d in [0u32, 1, 3] {
        let g = &by[(ts + d).to_string()]["getTimepoints"]["result"];
        // 4 массива по 2 значения: смещения (4 слова), затем [длина, конец окна, начало окна] каждого.
        let s = WindowStart {
            tick_cum: signed(g, 6).as_i64(),
            vol: word(g, 12).to::<u128>(),
            vpl: word(g, 15),
        };
        starts.push((ts + d, s));
    }
    let o = Oracle {
        cfg: config(&p["oracle"]["feeConfig"]["result"]),
        index,
        prev: around(p, index.wrapping_sub(1)),
        last: timepoint(&p["timepoints"]["result"]),
        next1: around(p, index.wrapping_add(1)),
        next2: around(p, index.wrapping_add(2)),
        zero: around(p, 0),
        vpl_in_block: 0,
        fee: gs_fee(p),
        starts,
    };
    (o, ts, tick)
}

#[test]
fn fee_port_equals_operator_get_fee() {
    let (mut cases, mut higher) = (0, 0);
    for p in pools(T237, 60) {
        let (o, ts, tick) = oracle(&p);
        let q = &p["quotes"][0];
        for (d, at) in [(0u32, "at_block"), (1, "at_plus_1"), (3, "at_plus_3")] {
            let b = &p["oracle"]["by_time"][(ts + d).to_string()];
            let gf = word(&b["getFee"]["result"], 0).to::<u32>();
            let ga = &b["getAverages"]["result"];
            let vol = word(ga, 0) / U256::from(15u8);
            let case = format!("{} +{d} с", p["pool"]);
            let formula = get_fee(vol, word(ga, 1), &o.cfg);
            assert_eq!(formula, Some(gf), "{case}: AdaptiveFee");
            let port = fee_at(&o, ts + d, tick);
            assert_eq!(port, Some(gf), "{case}: порт без объёма блока");
            let qf = word(&q[at]["quoter"]["result"], 1).to::<u32>();
            assert!(qf >= gf, "{case}: своп не ниже getFee");
            higher += usize::from(qf > gf);
            cases += 1;
        }
    }
    assert_eq!((cases, higher), (180, 81));
}

#[test]
fn quoter_fee_example_1b93() {
    let p = pools(T237, 60)
        .into_iter()
        .find(|p| p["pool"] == "0x1b932598da273069e1878e4a1505b6f833462210")
        .unwrap();
    assert_eq!(gs_fee(&p), 1537);
    let (o, ts, tick) = oracle(&p);
    for (d, at) in [(0u32, "at_block"), (1, "at_plus_1"), (3, "at_plus_3")] {
        let qf = word(&p["quotes"][0][at]["quoter"]["result"], 1).to::<u32>();
        assert_eq!(qf, 1542, "{at}");
        assert_eq!(fee_at(&o, ts + d, tick), Some(1542), "{at}");
    }
}
