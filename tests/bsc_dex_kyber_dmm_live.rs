//! Файл: tests/bsc_dex_kyber_dmm_live.rs
//! Что проверяет: решение 287 — KyberSwap Classic DMM (kyber_dmm) на РЕАЛЬНЫХ ответах узла BSC, записанных на одном блоке
//! 133682402 (tests/data/kyber_dmm_2026-11-15): четыре пула фабрики 0x878d…934c — USDT/WBNB ×1,5 0xec30…6381, USDT/BUSD
//! ×400 0xc3da…3b55, 0x3944…/BUSD без усиления 0xf8e6…9b31, USDC/0xe7c0… ×10 0xa23b…557a.
//! 1. Наша математика (load_dmm::state → math/dmm.rs::amount_out) на ответе getTradeInfo блока даёт ТОТ ЖЕ выход, что
//!    официальный роутер DMMRouter02.getAmountsOut — 24 из 24 (3 суммы × 2 стороны × 4 пула); обратная формула
//!    (amount_in) на этих выходах даёт вход, на котором прямая отдаёт не меньше.
//! 2. Реестр фабрики getPools(USDT, WBNB) — четыре пула: verify_rules::registered_for принимает пул из списка, чужой —
//!    отказ; у пары V2 — прежний разбор (registered).
//! 3. Шаг исполнителя: вид KYBER_DMM = 15, данных нет, только v2; проба на цепи (exec_sim.json): исполнитель v2 шагом 15
//!    получил ровно ответ роутера во всех 5 случаях (3 пула, обе стороны).
//! 4. Вид DEX: graphable, без своего пути (tradable = нет), шаг KYBER_DMM, нужны dex.factory и dex.quoter (роутер).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{math::dmm, pools::{load_dmm, families, verify_rules, events, exec_callback,
//! exec_kind}, exec::executor::kind}.

use alloy::sol_types::SolValue;
use alloy_primitives::{Address, U256};
use serde_json::Value;

use arb::core::types::{PoolKind, PoolState};
use arb::venues::bsc_dex::exec::executor::kind::Params;
use arb::venues::bsc_dex::math::dmm;
use arb::venues::bsc_dex::pools::exec_kind::ExecKind;
use arb::venues::bsc_dex::pools::{events, exec_callback, families, load_dmm, verify_rules};

const DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/data/kyber_dmm_2026-11-15/"
);

fn json(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(format!("{DIR}{name}")).unwrap()).unwrap()
}

fn addr(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}

fn dec(v: &Value) -> U256 {
    U256::from_str_radix(v.as_str().unwrap(), 10).unwrap()
}

/// Состояние пула по записанному getTradeInfo (тот же сборщик, что у чтения на узле).
fn state_of(p: &Value) -> PoolState {
    let t: Vec<U256> = p["getTradeInfo"]
        .as_array()
        .unwrap()
        .iter()
        .map(dec)
        .collect();
    load_dmm::state([t[0], t[1], t[2], t[3]], t[4]).expect("состояние")
}

/// Сторона обмена по направлению (0 — вход token0).
fn side(s: &PoolState, dir: u64) -> (dmm::Side, U256) {
    let PoolState::AmpV2(d) = s else {
        panic!("не AmpV2")
    };
    let side = if dir == 0 {
        dmm::Side {
            r_in: d.reserve0,
            r_out: d.reserve1,
            v_in: d.v_reserve0,
            v_out: d.v_reserve1,
        }
    } else {
        dmm::Side {
            r_in: d.reserve1,
            r_out: d.reserve0,
            v_in: d.v_reserve1,
            v_out: d.v_reserve0,
        }
    };
    (side, d.fee)
}

#[test]
fn formula_equals_router_24_of_24() {
    let probe = json("probe.json");
    let mut n = 0;
    for p in probe["pools"].as_array().unwrap() {
        let s = state_of(p);
        for q in p["quotes"].as_array().unwrap() {
            let (sd, fee) = side(&s, q["dir"].as_u64().unwrap());
            let amount = dec(&q["amount_in"]);
            let want = dec(&q["router_out"]);
            assert_eq!(
                dmm::amount_out(amount, sd, fee),
                Some(want),
                "{} {q}",
                p["pool"]
            );
            let back = dmm::amount_in(want, sd, fee).expect("обратная формула");
            assert!(dmm::amount_out(back, sd, fee).unwrap() >= want);
            n += 1;
        }
    }
    assert_eq!(n, 24);
}

#[test]
fn registry_get_pools() {
    let probe = json("probe.json");
    let list: Vec<Address> = probe["factory"]["getPools_USDT_WBNB"]
        .as_array()
        .unwrap()
        .iter()
        .map(addr)
        .collect();
    let answer = list.abi_encode();
    let fam = families::make("kyberswap_classic_bsc", "kyber_dmm").expect("вид");
    let pool = addr(&probe["pools"][0]["pool"]);
    assert!(verify_rules::registered_for(fam, pool, &answer).is_ok());
    let other = addr(&probe["pools"][1]["pool"]);
    assert!(verify_rules::registered_for(fam, other, &answer).is_err());
    // Не DMM — прежний разбор одного адреса (getPair): тот же ответ-массив не по ABI одного адреса или не тот адрес.
    let v2 = families::make("pancakeswap-v2", "pancakeswap_v2").expect("вид V2");
    assert!(verify_rules::registered_for(v2, pool, &answer).is_err());
}

#[test]
fn executor_step_on_chain_equals_router() {
    let sim = json("exec_sim.json");
    let cases = sim["execute"].as_array().unwrap();
    assert_eq!(cases.len(), 5);
    for c in cases {
        assert!(c["balance_set"].as_bool().unwrap());
        assert_eq!(dec(&c["received"]), dec(&c["router_out"]), "{c}");
    }
    let fam = families::make("kyberswap_classic_bsc", "kyber_dmm").expect("вид");
    assert!(fam.graphable && !fam.tradable && !fam.fork);
    assert_eq!(fam.kind, PoolKind::AmpV2);
    assert_eq!(fam.executor_kind, Some(ExecKind::KyberDmm));
    assert!(exec_callback::v2_only(ExecKind::KyberDmm));
    assert_eq!(Params::KyberDmm.kind(), 15);
    assert!(Params::KyberDmm.data().is_empty());
    assert_eq!(events::topics(fam).len(), 5);
    assert_eq!(events::liquidity_topics(fam).len(), 2);
}
