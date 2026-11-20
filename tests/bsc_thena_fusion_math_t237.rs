//! Файл: tests/bsc_thena_fusion_math_t237.rs
//! Что проверяет: решение 463 — математика бота THENA Fusion (math/swap_algebra, режим Row по сжатым строкам, карта по
//! сжатым словам — как load_algebra.rs) на второй записи своего узла 20.11 (tests/data/bsc_thena_fusion_2026-11-20_t237,
//! 60 пулов GeckoTerminal «thena-fusion», каждый на своём блоке 134742633…638; сжатых слов ±8, сырых ±1; скрипт
//! tools/bsc_record/thena_fusion.py ветки t237/bsc-thena-record 88e17fd2).
//! 1. Все 60 пулов × 8 котировок (1, 10, 100, 1000 б.п. резерва входа, обе стороны) × 4 времени (блок, время последней
//!    точки оракула, блок + 1 с, + 3 с): обмен бота с комиссией, которую вернул Quoter THENA на это время (второе
//!    слово ответа), = виртуальный квотер (вход и выход) и Quoter (выход) до wei — 1896 случаев; 24 случая (пулы
//!    0x117e…e3, 0x2f6c…98, 0x52dd…15, 0x987c…06 — по 4, 0xbf5e…3b — 8) уходят за записанные ±8 строк — честный
//!    неполный итог, выход меньше Quoter. Расхождений нет.
//! 2. На времени последней точки оракула Quoter вернул комиссию = globalState.fee у всех 60 (своп не пересчитывает её).
//! 3. Сырое толкование битмапа (tick >> 8, как бот до 463): у 58 пулов из 60 ни одного тика с ликвидностью.
//! 4. Пулы 0xd33f…362d и 0x489b…e98f — ни одного установленного бита ни в сырых, ни в сжатых ±8 словах (тики
//!    −92160…168900 и −61440…199620) при liquidity > 0: все позиции, дающие текущую ликвидность, шире окна в обе стороны
//!    (полный диапазон −887220…887220 с этим совместим, тики за окном не читались — не доказано). Обмен идёт по краям
//!    строк без пересечений, все 32 случая каждого — до wei.
//! Кто вызывает: cargo test.
//! От чего зависит: tests/thena_fusion_common; arb::venues::bsc_dex::math::swap_algebra.

mod thena_fusion_common;

use alloy_primitives::U256;

use arb::venues::bsc_dex::math::swap_algebra;

use thena_fusion_common::{T237, gs_fee, hex_u256, map_of, pools, state, word};

/// Времена котировок записи.
const AT: [&str; 4] = ["at_block", "at_last_timepoint", "at_plus_1", "at_plus_3"];
/// Пулы без единого установленного бита в записанных словах.
const EMPTY: [&str; 2] = [
    "0xd33f3c50df98d3bf0f47d8848340874a3bf9362d",
    "0x489bf777382306bc7de514fa0f1a4c725aebe98f",
];

#[test]
fn bot_math_equals_quoter_on_60_pools() {
    let (mut exact, mut partial) = (0, 0);
    for p in pools(T237, 60) {
        let m = map_of(&p);
        for q in p["quotes"].as_array().unwrap() {
            let zfo = q["zero_for_one"].as_bool().unwrap();
            let amount = hex_u256(&q["amount_in"]);
            for at in AT {
                let (qr, vr) = (&q[at]["quoter"]["result"], &q[at]["virtual"]["result"]);
                let (out, fee) = (word(qr, 0), word(qr, 1).to::<u32>());
                let (v_in, v_out) = (word(vr, 0), word(vr, 1));
                let case = format!("{} {at} zfo={zfo} bps={}", p["pool"], q["bps"]);
                assert_eq!(v_out, out, "{case}: виртуальный = Quoter");
                let s = state(&p, &m, fee);
                let r = swap_algebra::swap(&s, zfo, true, amount).expect("расчёт");
                if r.complete {
                    assert_eq!((r.amount_in, r.amount_out), (v_in, v_out), "{case}");
                    exact += 1;
                } else {
                    assert!(r.amount_out < out && r.amount_in < amount, "{case}");
                    partial += 1;
                }
            }
        }
    }
    assert_eq!((exact, partial), (1896, 24));
}

#[test]
fn fee_at_last_timepoint_is_global_state() {
    for p in pools(T237, 60) {
        for q in p["quotes"].as_array().unwrap() {
            let r = &q["at_last_timepoint"]["quoter"]["result"];
            assert_eq!(word(r, 1).to::<u32>(), gs_fee(&p), "{}", p["pool"]);
        }
    }
}

#[test]
fn raw_bitmap_is_empty_and_two_pools_have_no_ticks() {
    let mut raw_empty = 0;
    for p in pools(T237, 60) {
        let live = p["ticks"]
            .as_object()
            .unwrap()
            .values()
            .filter(|v| word(&v["result"], 0) > U256::ZERO)
            .count();
        if live == 0 {
            raw_empty += 1;
        }
        let name = p["pool"].as_str().unwrap();
        if EMPTY.contains(&name) {
            let m = map_of(&p);
            assert!(m.ticks.is_empty(), "{name}: в ±8 сжатых словах тиков нет");
            assert_eq!(live, 0, "{name}: в сырых словах тиков нет");
            let liq = word(&p["liquidity()"]["result"], 0);
            assert!(liq > U256::ZERO, "{name}");
        }
    }
    assert_eq!(raw_empty, 58, "сырых тиков нет у 58 пулов из 60");
}
