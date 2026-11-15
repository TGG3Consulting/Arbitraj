//! Файл: tests/bsc_dex_smardex_live.rs
//! Что проверяет: решение 288 — SmarDex (smardex) на РЕАЛЬНЫХ ответах узла BSC, записанных на одном блоке 133685263 (время
//! блока 1794723017; tests/data/smardex_2026-11-15): четыре пары фабрики 0xA8EF…Af33 — USDT/SDEX 0xe7b8…7ab2,
//! BTCB/LXIUM 0x4357…2985, BTCB/SDEX 0xc5dc…3872, LXIUM/WBNB 0x07df…66d1.
//! 1. Наша математика (load_smardex::state → math/smardex_swap.rs::swap_out со средней ценой к времени блока) даёт ТОТ ЖЕ выход,
//!    что пара своим кодом (виртуальный квотер quoteSmardex на том же блоке) — 24 из 24 (3 суммы × 2 стороны × 4 пары).
//! 2. Время блока раньше записи средней цены — состояние не собирается (обмен откатился бы).
//! 3. Шаг исполнителя: вид SMARDEX = 16, данных нет, только v2; проба на цепи (exec_sim.json): исполнитель v2 шагом 16
//!    получил ровно ответ квотера во всех 6 случаях (4 пары, обе стороны).
//! 4. Вид DEX: graphable, без своего пути (tradable = нет), шаг SMARDEX, вид ядра Fictive; события пары — 5 topic0.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{math::smardex_swap, pools::{load_smardex, families, events,
//! exec_callback, exec_kind}, exec::executor::kind}.

use alloy_primitives::U256;
use serde_json::Value;

use arb::core::types::{PoolKind, PoolState};
use arb::venues::bsc_dex::exec::executor::kind::Params;
use arb::venues::bsc_dex::math::smardex_swap;
use arb::venues::bsc_dex::pools::exec_kind::ExecKind;
use arb::venues::bsc_dex::pools::{events, exec_callback, families, load_smardex};

const DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/data/smardex_2026-11-15/"
);

fn json(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(format!("{DIR}{name}")).unwrap()).unwrap()
}

fn dec(v: &Value) -> U256 {
    U256::from_str_radix(v.as_str().unwrap(), 10).unwrap()
}

fn pair(v: &Value, k: &str, i: usize) -> U256 {
    dec(&v[k][i])
}

/// Сырые ответы пары на блоке (тот же сборщик, что у чтения на узле).
fn raw(p: &Value, block_ts: u64) -> load_smardex::Raw {
    load_smardex::Raw {
        reserves: (pair(p, "getReserves", 0), pair(p, "getReserves", 1)),
        fictive: (
            pair(p, "getFictiveReserves", 0),
            pair(p, "getFictiveReserves", 1),
        ),
        price: (pair(p, "getPriceAverage", 0), pair(p, "getPriceAverage", 1)),
        price_ts: pair(p, "getPriceAverage", 2),
        fees: (pair(p, "getPairFees", 0), pair(p, "getPairFees", 1)),
        block_ts: U256::from(block_ts),
    }
}

#[test]
fn formula_equals_pair_24_of_24() {
    let probe = json("probe.json");
    let ts = probe["block_timestamp"].as_u64().unwrap();
    let mut n = 0;
    for p in probe["pairs"].as_array().unwrap() {
        let PoolState::Fictive(d) = load_smardex::state(&raw(p, ts)).expect("состояние")
        else {
            panic!("не Fictive")
        };
        for q in p["quotes"].as_array().unwrap() {
            let side = smardex_swap::side(&d, q["dir"].as_u64().unwrap() == 0);
            let amount = dec(&q["amount_in"]);
            assert_eq!(dec(&q["vq_used"]), amount);
            let got = smardex_swap::swap_out(&side, d.price_average_ts, d.block_ts, amount);
            assert_eq!(got, Some(dec(&q["vq_out"])), "{} {q}", p["pool"]);
            n += 1;
        }
    }
    assert_eq!(n, 24);
}

#[test]
fn block_time_before_price_average_rejected() {
    let probe = json("probe.json");
    let p = &probe["pairs"][0];
    let last = p["getPriceAverage"][2]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    assert!(load_smardex::state(&raw(p, last - 1)).is_err());
    assert!(load_smardex::state(&raw(p, last)).is_ok());
}

#[test]
fn executor_step_on_chain_equals_quoter() {
    let sim = json("exec_sim.json");
    let cases = sim["execute"].as_array().unwrap();
    assert_eq!(cases.len(), 6);
    for c in cases {
        assert_eq!(dec(&c["received"]), dec(&c["quoteSmardex_out"]), "{c}");
    }
    let fam = families::make("smardex-bsc", "smardex").expect("вид");
    assert!(fam.graphable && !fam.tradable && !fam.fork);
    assert_eq!(fam.kind, PoolKind::Fictive);
    assert_eq!(fam.executor_kind, Some(ExecKind::Smardex));
    assert!(exec_callback::v2_only(ExecKind::Smardex));
    assert_eq!(Params::Smardex.kind(), 16);
    assert!(Params::Smardex.data().is_empty());
    assert_eq!(events::topics(fam).len(), 5);
    assert_eq!(events::liquidity_topics(fam).len(), 2);
}
