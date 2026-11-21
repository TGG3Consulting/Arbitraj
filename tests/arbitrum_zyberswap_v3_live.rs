//! Файл: tests/arbitrum_zyberswap_v3_live.rs
//! Что проверяет: решение 487 (T487-01, T487-02) — Zyberswap V3 (Arbitrum) видом thena_fusion на записи своего узла
//! Arbitrum (tools/arbitrum_record/zyberswap_v3.py → tests/data/arbitrum_zyberswap_v3_<дата>/; формат — как записи
//! THENA Fusion BSC, tools/bsc_record/thena_fusion.py). Официального квотера у Zyberswap нет — сверка с виртуальным
//! квотером (обмен самого пула в eth_call) и с getFee оператора пула:
//! 1. pools_from_factory — пул = poolByPair фабрики, tickSpacing = 60, CREATE2 от poolDeployer() фабрики по хешу кода
//!    0x6ec6…94a4 (POOL_INIT_CODE_HASH фабрики, Sourcify 42161) — пул из проверенного кода;
//! 2. fee_without_block_volume_equals_get_fee — комиссия бота (fee_algebra_v1::fee_at, volumePerLiquidityInBlock = 0)
//!    = getFee оператора на время блока + 0 / 1 / 3 с;
//! 3. swap_with_bot_fee_equals_pool — обмен бота (сжатая карта, комиссия fee_at со слотом 3) = вход и выход
//!    виртуального квотера на всех 4 временах; неполный — только у самого пула;
//! 4. next_block_same_or_next_second — T487-02: времена соседних блоков heads.json отличаются на 0 или 1 с
//!    (next_block_fee берёт время блока и + 1 с); не так — тест падает с числом, вопрос к коду.
//!
//! Под #[ignore] до записи (запись делает Тигран своим узлом Arbitrum). Кто вызывает: cargo test.
//! От чего зависит: tests/thena_fusion_common (oracle); arb::venues::bsc_dex::math::{fee_algebra_v1, swap_algebra}.

mod thena_fusion_common;

use alloy_primitives::{Address, U256, keccak256};
use serde_json::Value;

use arb::venues::bsc_dex::math::fee_algebra_v1::fee_at;
use arb::venues::bsc_dex::math::swap_algebra;

use thena_fusion_common::oracle::oracle;
use thena_fusion_common::{file, hex_u256, latest, map_of, pools_all, state, word};

/// Хеш кода пула Zyberswap V3 (AlgebraFactory.sol POOL_INIT_CODE_HASH, Sourcify 42161 0x9c2a…8544; решение 487).
const INIT_HASH: &str = "6ec6c9c8091d160c0aa74b2b14ba9c1717e95093bd3ac085cee99a49aab294a4";

/// Времена котировок записи и сдвиг от времени блока (None — время последней точки оракула).
const AT: [(&str, Option<u32>); 4] = [
    ("at_block", Some(0)),
    ("at_last_timepoint", None),
    ("at_plus_1", Some(1)),
    ("at_plus_3", Some(3)),
];

/// Папка последней записи. Ошибок нет (тест падает: записи нет).
fn rec() -> String {
    latest("arbitrum_zyberswap_v3_").expect("нет записи tools/arbitrum_record/zyberswap_v3.py")
}

/// Адрес из hex-строки записи. Ошибок нет (тест падает).
fn address(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}

#[test]
#[ignore = "нет записи: tools/arbitrum_record/zyberswap_v3.py (T487-01)"]
fn pools_from_factory() {
    let r = rec();
    let fac = file(&r, "factory.json");
    let deployer = address(&fac["poolDeployer"]);
    let hash: [u8; 32] = alloy_primitives::hex::decode(INIT_HASH)
        .unwrap()
        .try_into()
        .unwrap();
    for p in pools_all(&r) {
        let pool = address(&p["pool"]);
        assert_eq!(
            Address::from_word(word(&p["poolByPair"]["result"], 0).into()),
            pool
        );
        assert_eq!(p["tick_spacing"].as_i64(), Some(60), "{pool}");
        let (t0, t1) = (address(&p["token0"]), address(&p["token1"]));
        let salt = keccak256([t0.into_word().0, t1.into_word().0].concat());
        assert_eq!(deployer.create2(salt, hash), pool, "CREATE2 {pool}");
    }
}

#[test]
#[ignore = "нет записи: tools/arbitrum_record/zyberswap_v3.py (T487-01)"]
fn fee_without_block_volume_equals_get_fee() {
    for p in pools_all(&rec()) {
        let (o, ts, tick, _) = oracle(&p, false);
        for d in [0u32, 1, 3] {
            let b = &p["oracle"]["by_time"][(ts + d).to_string()];
            let gf = word(&b["getFee"]["result"], 0).to::<u32>();
            assert_eq!(fee_at(&o, ts + d, tick), Some(gf), "{} +{d} с", p["pool"]);
        }
    }
}

#[test]
#[ignore = "нет записи: tools/arbitrum_record/zyberswap_v3.py (T487-01)"]
fn swap_with_bot_fee_equals_pool() {
    for p in pools_all(&rec()) {
        let (o, ts, tick, _) = oracle(&p, true);
        let m = map_of(&p);
        for q in p["quotes"].as_array().unwrap() {
            let zfo = q["zero_for_one"].as_bool().unwrap();
            let amount = hex_u256(&q["amount_in"]);
            for (at, d) in AT {
                let vr = &q[at]["virtual"]["result"];
                let (v_in, v_out) = (word(vr, 0), word(vr, 1));
                let case = format!("{} {at} zfo={zfo} bps={}", p["pool"], q["bps"]);
                let t = d.map_or(o.last.ts, |d| ts + d);
                let fee = fee_at(&o, t, tick).expect("комиссия");
                let r = swap_algebra::swap(&state(&p, &m, fee), zfo, true, amount).expect("расчёт");
                assert_eq!((r.amount_in, r.amount_out), (v_in, v_out), "{case}");
                if !r.complete {
                    assert!(v_in < amount, "{case}: пул взял всю сумму, а бот — нет");
                }
            }
        }
    }
}

#[test]
#[ignore = "нет записи: tools/arbitrum_record/zyberswap_v3.py (T487-01, T487-02)"]
fn next_block_same_or_next_second() {
    let h = file(&rec(), "heads.json");
    let ts: Vec<U256> = h
        .as_array()
        .unwrap()
        .iter()
        .map(|b| hex_u256(&b["timestamp"]))
        .collect();
    assert!(ts.len() > 1, "в heads.json меньше двух блоков");
    let gaps: Vec<U256> = ts.windows(2).map(|w| w[1] - w[0]).collect();
    let max = gaps.iter().max().unwrap();
    assert!(
        *max <= U256::from(1u8),
        "T487-02: между соседними блоками до {max} с — next_block_fee берёт только время блока и + 1 с"
    );
}
