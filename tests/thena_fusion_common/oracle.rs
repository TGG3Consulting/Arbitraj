//! Файл: tests/thena_fusion_common/oracle.rs
//! Что делает: решение 487 — разбор данных оракула комиссии записи кода AlgebraV1 (точки timepoints, feeConfig()
//! оператора, начало окна getTimepoints, слот 3 — volumePerLiquidityInBlock) в math/fee_algebra_v1::Oracle — те же
//! функции вынесены из tests/bsc_thena_fusion_fee_slot3.rs (ревью t256); запись Zyberswap V3 Arbitrum
//! (tests/arbitrum_zyberswap_v3_live.rs) — того же формата (tools/bsc_record/thena_fusion.py).
//! Кто вызывает: tests/bsc_thena_fusion_fee_slot3.rs, tests/arbitrum_zyberswap_v3_live.rs.
//! От чего зависит: thena_fusion_common (word, signed, len, hex_u256, gs_fee); arb::venues::bsc_dex::math::{adaptive_fee,
//! fee_algebra_v1}.
//! Настройки: нет.

use alloy_primitives::{I256, U256};
use serde_json::Value;

use arb::venues::bsc_dex::math::adaptive_fee::FeeConfig;
use arb::venues::bsc_dex::math::fee_algebra_v1::{Oracle, Timepoint, WindowStart};

use super::{gs_fee, hex_u256, len, signed, word};

/// Точка оракула из ответа timepoints(uint256) (7 слов). Ошибок нет (тест падает).
pub fn timepoint(r: &Value) -> Timepoint {
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
pub fn config(r: &Value) -> FeeConfig {
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
pub fn arr(r: &Value, j: usize, k: usize) -> U256 {
    let at = word(r, j).to::<usize>() / 32;
    assert!(word(r, at).to::<usize>() > k, "длина массива {j}");
    word(r, at + 1 + k)
}

/// Начало окна из ответа getTimepoints: значение `k`. Ошибок нет (тест падает).
pub fn start(r: &Value, k: usize) -> WindowStart {
    WindowStart {
        tick_cum: I256::from_raw(arr(r, 0, k)).as_i64(),
        vol: arr(r, 2, k).to::<u128>(),
        vpl: arr(r, 3, k),
    }
}

/// Оракул записи (`block_vpl` — volumePerLiquidityInBlock слота 3; false — 0). Возвращает: оракул, время блока, тик,
/// volumePerLiquidityInBlock. Ошибок нет (тест падает).
pub fn oracle(p: &Value, block_vpl: bool) -> (Oracle, u32, i32, u128) {
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
