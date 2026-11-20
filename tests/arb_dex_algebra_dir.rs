//! Файл: tests/arb_dex_algebra_dir.rs
//! Что проверяет: решение 444 (T433-F5) — вид algebra_dir_fee (Algebra V1.9 с направленными комиссиями, Camelot V3)
//! на записи своего узла Arbitrum (tools/arbitrum_record/algebra_dir_fee.py → tests/data/arbitrum_algebra_dir_fee_*):
//! 1. Версия кода пулов фабрики: адрес каждого пула = CREATE2(poolDeployer, keccak256(abi.encode(token0, token1)),
//!    POOL_INIT_CODE_HASH) для хеша версии с направленными комиссиями и СЫРЫМ битмапом (как читает load_algebra.rs);
//!    poolByPair фабрики = этот пул.
//! 2. Расчёт обмена бота (math/swap_algebra + комиссия стороны math/algebra_fee) = ответу виртуального квотера (обмен
//!    самого пула в eth_call) и официального Quoter v1, если он ответил, — при времени блока = времени последней точки
//!    оракула пула (обмен тогда не пересчитывает комиссии, AlgebraPool.sol:777-795), обе стороны, все суммы записи.
//!    Quoter v1 отдаёт и комиссию обмена — она равна комиссии стороны из globalState.
//! Тесты с меткой ignore до записи (метку снять после записи — docs/TASKS.md, «Решение 444»).
//! Кто вызывает: cargo test.
//! От чего зависит: tests/algebra_dir_common, arb::venues::bsc_dex::{math::{swap_algebra, algebra_fee},
//! quote::virtual_quoter}.

mod algebra_dir_common;

use std::collections::HashSet;

use alloy_primitives::{Address, B256, U256, keccak256};

use arb::core::types::PoolState;
use arb::venues::bsc_dex::math::algebra_fee::swap_fee;
use arb::venues::bsc_dex::math::swap_algebra::{self, AlgebraState};
use arb::venues::bsc_dex::quote::virtual_quoter;

use algebra_dir_common::{json, pool_state, pools, result};

/// Хеши кода пула ветки direction-based-fee с feeZto/feeOtz и сырым битмапом (tools/arbitrum_record/README.md,
/// таблица: коммиты 4ada8b0, 237524b, 4c885a5, cd4fd3b, c63c18f…a45417e).
const RAW_DIR: [&str; 5] = [
    "0xac97ff1b66ef7517262cbf16db0195b400714e8ecb499fc86f349e77f50ae89b",
    "0x0ca66607e4d0ece7798fce9ae13cad1a28be1eee443451573f92ba6e3bcd92b9",
    "0x5d100e2dbf4b80b9451c29380def0d6c4abeb3db0d166c81fda155a89e106dbd",
    "0xab9acbcae566bd662fb801895320dc3dd170e64394f78600b969ab128a3a40fd",
    "0x6c1bebd370ba84753516bc1393c0d0a6c645856da55f5393ac8ab3d6dbc861d3",
];

/// Адрес из строки записи. Ошибок нет (тест падает).
fn addr(v: &serde_json::Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}

/// Число из hex-строки записи. Ошибок нет (тест падает).
fn hex_u256(v: &serde_json::Value) -> U256 {
    let h = v.as_str().unwrap().trim_start_matches("0x");
    U256::from_str_radix(h, 16).unwrap()
}

#[test]
#[ignore = "нет записи: tools/arbitrum_record/algebra_dir_fee.py"]
fn pools_are_directional_raw_bitmap_code() {
    let fac = json("factory.json");
    let deployer = addr(&fac["poolDeployer"]);
    let ok: HashSet<B256> = RAW_DIR.iter().map(|h| h.parse().unwrap()).collect();
    for p in pools() {
        let pool = addr(&p["pool"]);
        let (t0, t1) = (addr(&p["token0"]), addr(&p["token1"]));
        let mut enc = [0u8; 64];
        enc[12..32].copy_from_slice(t0.as_slice());
        enc[44..64].copy_from_slice(t1.as_slice());
        let salt = keccak256(enc);
        let hit: Vec<B256> = ok
            .iter()
            .filter(|h| deployer.create2(salt, **h) == pool)
            .copied()
            .collect();
        assert_eq!(
            hit.len(),
            1,
            "{pool}: код пула не из версий с направленными комиссиями и сырым битмапом"
        );
        let by_pair = result(&p["poolByPair"]).unwrap();
        assert_eq!(Address::from_slice(&by_pair[12..32]), pool, "poolByPair фабрики");
    }
}

#[test]
#[ignore = "нет записи: tools/arbitrum_record/algebra_dir_fee.py"]
fn math_equals_pool_swap() {
    let mut compared = 0;
    for p in pools() {
        let (state, map) = pool_state(&p);
        let PoolState::Algebra {
            sqrt_price_x96,
            tick,
            liquidity,
            fee,
            fee_otz,
        } = state
        else {
            panic!("не Algebra");
        };
        for q in p["quotes"].as_array().unwrap() {
            let zfo = q["zero_for_one"].as_bool().unwrap();
            let amount = hex_u256(&q["amount_in"]);
            let f = swap_fee(fee, fee_otz, zfo);
            let s = AlgebraState {
                sqrt_price: sqrt_price_x96,
                tick,
                liquidity,
                fee: f,
                ticks: &map,
            };
            let ours = swap_algebra::swap(&s, zfo, true, amount).unwrap();
            let at = &q["at_last_timepoint"];
            let pool = &p["pool"];
            // Расчёт вышел за прочитанное окно карты (--words записи) — честный неполный итог, сравнивать нечего.
            if !ours.complete {
                continue;
            }
            if let Some(raw) = result(&at["virtual"]) {
                let out = virtual_quoter::decode(&raw, amount).unwrap();
                assert_eq!(out, Ok(ours.amount_out), "{pool} zfo={zfo} {amount}");
                compared += 1;
            }
            if let Some(raw) = at.get("quoter_v1").and_then(result) {
                let out = U256::from_be_slice(&raw[..32]);
                let qfee = U256::from_be_slice(&raw[32..64]).to::<u32>();
                assert_eq!(qfee, f, "{pool}: комиссия Quoter v1 ≠ комиссии стороны");
                assert_eq!(ours.amount_out, out, "{pool} Quoter v1 zfo={zfo} {amount}");
            }
        }
    }
    assert!(compared > 0, "ни одного сравнения с обменом пула");
}
