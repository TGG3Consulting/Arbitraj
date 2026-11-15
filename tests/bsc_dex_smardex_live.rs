//! Файл: tests/bsc_dex_smardex_live.rs
//! Что проверяет: решение 288 — SmarDex (smardex) на РЕАЛЬНЫХ ответах узла BSC, записанных на одном блоке 133685263 (время
//! блока 1794723017; tests/data/smardex_2026-11-15): четыре пары фабрики 0xA8EF…Af33 — USDT/SDEX 0xe7b8…7ab2,
//! BTCB/LXIUM 0x4357…2985, BTCB/SDEX 0xc5dc…3872, LXIUM/WBNB 0x07df…66d1.
//! 1. Наша математика (load_smardex::state → math/smardex_swap.rs::swap_out со средней ценой к времени блока) даёт ТОТ ЖЕ выход,
//!    что пара своим кодом (виртуальный квотер quoteSmardex на том же блоке) — 24 из 24 (3 суммы × 2 стороны × 4 пары).
//! 2. Время блока раньше записи средней цены — состояние не собирается (обмен откатился бы).
//! 3. Ветки по времени (probe_time.json, блок 133694132): dt = 0 и 0 < dt < 300 (переопределение времени блока в
//!    eth_call), деление на два шага, пары без записи средней цены — наша формула = выход пары в 44 из 46; в 2 (пыльная
//!    пара 0x4460…5071, вход token1) новый фиктивный резерв = 0: квотер видит выход в обратном вызове, а обмен откатился
//!    бы в _update (SmardexPair.sol L358) — наша формула даёт None, выход getAmountOut = выход квотера.
//! 4. Шаг исполнителя на цепи: выход шага 16 исполнителя v2 (exec_sim.json) = наша формула на состоянии пары того же
//!    блока (probe.json) — 6 из 6. Вид SMARDEX = 16, данных нет, только v2.
//! 5. Вид DEX: graphable, без своего пути (tradable = нет), шаг SMARDEX, вид ядра Fictive; события пары — 5 topic0.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{math::smardex_swap, pools::{load_smardex, families, events,
//! exec_callback, exec_kind}, exec::executor::kind}.

use alloy_primitives::U256;
use serde_json::Value;

use arb::core::types::{PoolKind, PoolState};
use arb::venues::bsc_dex::exec::executor::kind::Params;
use arb::venues::bsc_dex::math::smardex::{self, Params as SdxParams};
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

/// Выход по нашей формуле на записанном состоянии пары: вход token_in, время блока now. Возвращает: выход.
fn ours(p: &Value, now: u64, zero_for_one: bool, amount: U256) -> Option<U256> {
    let PoolState::Fictive(d) = load_smardex::state(&raw(p, now)).expect("состояние")
    else {
        panic!("не Fictive")
    };
    let side = smardex_swap::side(&d, zero_for_one);
    smardex_swap::swap_out(&side, d.price_average_ts, d.block_ts, amount)
}

#[test]
fn formula_equals_pair_time_branches() {
    // Блок 133694132 (probe_time.json): окно 0 ≤ dt < 300 (время блока переопределено: dt = 0 и dt = 120 с), деление
    // обмена на два шага и пары без записи средней цены (last_ts = 0) — сверка с выходом пары своим кодом.
    let st = json("probe_time.json");
    let (mut n, mut reverted) = (0, 0);
    let mut seen = std::collections::BTreeSet::new();
    for p in st["pairs"].as_array().unwrap() {
        for q in p["quotes"].as_array().unwrap() {
            let amount = dec(&q["amount_in"]);
            assert_eq!(dec(&q["vq_used"]), amount);
            let (now, zf) = (q["time"].as_u64().unwrap(), q["dir"].as_u64() == Some(0));
            match ours(p, now, zf, amount) {
                Some(got) => assert_eq!(got, dec(&q["vq_out"]), "{} {q}", p["pool"]),
                None => {
                    // Квотер читает выход в обратном вызове (SmardexPair.sol L268 / L279), ДО _update (L296 / L308) с
                    // require фиктивных резервов != 0 (L358). Наша формула отдаёт None (обмен откатился бы) — проверяем,
                    // что выход getAmountOut совпал с квотером и откат именно из-за нулевого фиктивного резерва.
                    let PoolState::Fictive(d) =
                        load_smardex::state(&raw(p, now)).expect("состояние")
                    else {
                        panic!("не Fictive")
                    };
                    let s = smardex_swap::side(&d, zf);
                    let (pi, po) = smardex_swap::price_average(
                        (s.fictive_in, s.fictive_out),
                        d.price_average_ts,
                        (s.price_in, s.price_out),
                        d.block_ts,
                    )
                    .expect("средняя цена");
                    let s = SdxParams {
                        price_in: pi,
                        price_out: po,
                        ..s
                    };
                    let (out, f_in, f_out) = smardex::amount_out(&s, amount).expect("выход");
                    assert_eq!(out, dec(&q["vq_out"]), "{} {q}", p["pool"]);
                    assert!(f_in.is_zero() || f_out.is_zero(), "{} {q}", p["pool"]);
                    reverted += 1;
                }
            }
            seen.insert(q["branch"].as_str().unwrap().to_string());
            n += 1;
        }
    }
    assert_eq!((n, reverted), (46, 2));
    for b in [
        "dt0+single",
        "dt0+two_step",
        "dtwin+single",
        "dtwin+two_step",
        "last0+single",
        "last0+recompute",
    ] {
        assert!(seen.contains(b), "ветка {b}");
    }
}

#[test]
fn executor_step_on_chain_equals_formula() {
    // Блок 133685263: состояние пар — probe.json, шаг 16 исполнителя v2 (exec_sim.json) на том же блоке.
    let probe = json("probe.json");
    let ts = probe["block_timestamp"].as_u64().unwrap();
    let sim = json("exec_sim.json");
    let cases = sim["execute"].as_array().unwrap();
    assert_eq!(cases.len(), 6);
    for c in cases {
        let p = probe["pairs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["pool"] == c["pool"])
            .expect("пара в probe.json");
        let zf = c["token_in"] == p["token0"];
        let got = ours(p, ts, zf, dec(&c["balance_in"]));
        assert_eq!(got, Some(dec(&c["received"])), "{c}");
    }
}

#[test]
fn family_and_step() {
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
