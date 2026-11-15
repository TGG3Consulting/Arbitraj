//! Файл: tests/bsc_dex_nomiswap_stable_live.rs
//! Что проверяет: решение 284 — Nomiswap Stable (nomiswap_stable) на РЕАЛЬНЫХ ответах узла BSC, записанных на одном
//! блоке 133650877 (tests/data/nomiswap_stable_2026-11-15): три пары фабрики 0xc6b7…6285 — USDT/USDC 0x1e40…60f1,
//! USDT/BUSD 0x3a66…da87, USDC/BUSD 0xca25…a7ec.
//! 1. Наша математика (load_nomi::state → math/stableswap_nomi.rs::amount_out) на резервах, swapFee, getA и
//!    множителях блока даёт ТОТ ЖЕ выход, что getAmountOut(address, uint256) пары — 24 из 24 (4 суммы × 2 стороны × 3).
//! 2. Проверка пары путём V2: поля factory/token0/token1 (verify_rules::fill/check) и реестр getPair фабрики
//!    (verify_rules::registered) принимают пару; чужая монета — отказ.
//! 3. Реальные логи Sync / Swap пары — в topics() вида; RampA / StopRampA / Mint / Burn — тоже.
//! 4. Шаг исполнителя: вид NOMI_STABLE = 12, данных нет; только v2 (exec_callback::v2_only); проба на цепи
//!    (exec_sim.json): исполнитель v2 шагом 12 получил ровно getAmountOut пары на 1000 USDT, и наша математика — то же.
//! 5. Вид DEX: graphable, без своего пути (tradable = нет), шаг NOMI_STABLE, нужна только dex.factory.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{math::stableswap_nomi, pools::{load_nomi, families, verify_rules, events,
//! exec_callback, exec_kind}, exec::executor::kind}.

use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;

use arb::core::types::PoolState;
use arb::venues::bsc_dex::exec::executor::kind::Params;
use arb::venues::bsc_dex::math::stableswap_nomi;
use arb::venues::bsc_dex::pools::exec_kind::ExecKind;
use arb::venues::bsc_dex::pools::verify::Candidate;
use arb::venues::bsc_dex::pools::{events, exec_callback, families, load_nomi, verify_rules};

const DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/data/nomiswap_stable_2026-11-15/"
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

/// Слово `i` сырого ответа.
fn word(raw: &str, i: usize) -> U256 {
    let h = raw.trim_start_matches("0x");
    U256::from_str_radix(&h[64 * i..64 * (i + 1)], 16).unwrap()
}

/// Состояние пары по записанным ответам (тот же сборщик, что у чтения на узле).
fn state_of(p: &Value) -> PoolState {
    let r = p["getReserves"].as_str().unwrap();
    load_nomi::state(
        addr(&p["token0"]),
        (addr(&p["token0"]), addr(&p["token1"])),
        (word(r, 0), word(r, 1)),
        p["swapFee"].as_str().unwrap().parse().unwrap(),
        dec(&p["getA"]),
        (
            p["token0PrecisionMultiplier"].as_str().unwrap().parse().unwrap(),
            p["token1PrecisionMultiplier"].as_str().unwrap().parse().unwrap(),
        ),
    )
    .expect("состояние")
}

#[test]
fn math_equals_pair_get_amount_out() {
    let probe = json("probe.json");
    let mut n = 0;
    for p in probe["pairs"].as_array().unwrap() {
        let PoolState::StableSwap(d) = state_of(p) else {
            panic!("не StableSwap")
        };
        assert!(d.input_fee && !d.strict && d.ng.is_none());
        for g in p["getAmountOut"].as_array().unwrap() {
            let side = g[0].as_u64().unwrap() as usize;
            let (i, j) = if side == 0 { (0, 1) } else { (1, 0) };
            let (dx, want) = (dec(&g[1]), dec(&g[2]));
            let got = stableswap_nomi::amount_out(
                i,
                j,
                dx,
                &d.balances,
                &d.rates,
                d.amp,
                d.fee,
                d.amp_precision,
                d.max_rounds,
            )
            .expect("расчёт");
            assert_eq!(got, want, "{} {i}→{j} dx={dx}", p["pair"]);
            n += 1;
        }
    }
    assert_eq!(n, 24);
}

#[test]
fn pair_verified_by_v2_path() {
    let probe = json("probe.json");
    let fam = families::make("nomiswap_stable", "nomiswap_stable").expect("вид");
    let factory = addr(&probe["factory"]);
    for p in probe["pairs"].as_array().unwrap() {
        let pool = addr(&p["pair"]);
        let (t0, t1) = (addr(&p["token0"]), addr(&p["token1"]));
        let w = |a: Address| Ok(Bytes::from(a.into_word().to_vec()));
        let answers = vec![w(addr(&p["factory"])), w(t0), w(t1)];
        let mut read = verify_rules::Read::default();
        verify_rules::fill(fam, &mut read, &answers).expect("поля пары");
        let cand = |coin: Address, quote: Address| Candidate {
            family: fam,
            pool: p["pair"].as_str().unwrap().to_string(),
            coin,
            quote,
        };
        verify_rules::check(&cand(t0, t1), factory, &read).expect("монеты и фабрика");
        assert!(verify_rules::check(&cand(pool, t1), factory, &read).is_err());
        let reg = addr(&p["getPair"]).into_word().to_vec();
        verify_rules::registered(pool, &reg).expect("реестр getPair");
        assert!(verify_rules::registered(factory, &reg).is_err());
    }
}

#[test]
fn real_logs_in_topics() {
    let probe = json("probe.json");
    let fam = families::make("nomiswap_stable", "nomiswap_stable").expect("вид");
    let topics = events::topics(fam);
    for l in probe["logs"].as_array().unwrap() {
        let t0: alloy_primitives::B256 = l["topics"][0].as_str().unwrap().parse().unwrap();
        assert!(topics.contains(&t0), "{t0}");
    }
    assert_eq!(topics.len(), 6);
}

#[test]
fn executor_step_and_family() {
    let fam = families::make("nomiswap_stable", "nomiswap_stable").expect("вид");
    assert!(fam.graphable && !fam.tradable && !fam.fork);
    assert_eq!(fam.executor_kind, Some(ExecKind::NomiStable));
    assert!(exec_callback::v2_only(ExecKind::NomiStable));
    assert!(!exec_callback::v2_only(ExecKind::Solidly));
    assert_eq!(Params::NomiStable.kind(), 12);
    assert!(Params::NomiStable.data().is_empty());
    // Проба на цепи: шаг 12 исполнителя v2 через пару USDT/USDC на 1000 USDT = getAmountOut = наша математика.
    let sim = json("exec_sim.json");
    let probe = json("probe.json");
    let got = dec(&sim["nomi_step_kind12_received"]);
    assert_eq!(got, dec(&sim["pair_getAmountOut_usdt_1000e18"]));
    let PoolState::StableSwap(d) = state_of(&probe["pairs"][0]) else {
        panic!("не StableSwap")
    };
    let ours = stableswap_nomi::amount_out(
        0,
        1,
        U256::from(1000u64) * U256::from(10u64).pow(U256::from(18u8)),
        &d.balances,
        &d.rates,
        d.amp,
        d.fee,
        d.amp_precision,
        d.max_rounds,
    );
    assert_eq!(ours, Some(got));
}
