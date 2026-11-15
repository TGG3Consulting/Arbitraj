//! Файл: tests/bsc_dex_balancer_acsi_live.rs
//! Что проверяет: решение 289 — Balancer V2 (balancer_v2, ACSI Finance BSC) на РЕАЛЬНЫХ ответах узла BSC, записанных на
//! одном блоке 133699740 (tests/data/balancer_acsi_2026-11-15): хранилище 0xa82f…ceced, пять весовых пулов (3 и 4 токена
//! MINIMAL_SWAP_INFO, 2 токена TWO_TOKEN), три стейблсвап-пула (GENERAL) и пул 0xb978…59ee не из фабрик.
//! 1. Наша математика (load_balancer::state → math/balancer_math.rs) на записанном состоянии даёт ТОТ ЖЕ выход, что
//!    Vault.queryBatchSwap GIVEN_IN, во всех ответах пулов фабрик (включая откаты: вход 35 % баланса весового — BAL#304,
//!    у стейблсвапа 0xe901…c6c5 — несходимость BAL#322 → None), и тот же вход, что GIVEN_OUT.
//! 2. Проверка пула (verify_balancer): пулы фабрик принимаются; пул не из фабрик, самозванец (хранилище знает под id
//!    другой адрес), чужое хранилище, пара не из токенов пула, стейблсвап без dex.balancer_stable_factory — отказ.
//! 3. Шаг исполнителя на цепи: выход шага 17 исполнителя v2 (eth_call с подменой кода, exec_sim.json) = наша формула на
//!    состоянии того же блока — 9 одиночных шагов и маршрут из двух шагов; разбор ответа queryBatchSwap — quoter_balancer.
//! 4. Вид BALANCER = 17, data = poolId, только v2; вид DEX — graphable, шаг BALANCER; topic0 событий пула — по подписям.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{math::balancer_math, pools::{load_balancer, verify_balancer, families,
//! exec_kind, exec_callback, abi_balancer}, quote::quoter_balancer, exec::executor::kind}.

use alloy::sol_types::{SolCall, SolValue};
use alloy_primitives::{Address, B256, Bytes, I256, U256, keccak256};
use serde_json::Value;

use arb::core::types::{PoolKind, PoolState, VaultCurve, VaultPoolData};
use arb::venues::bsc_dex::exec::executor::kind::Params;
use arb::venues::bsc_dex::math::balancer_math;
use arb::venues::bsc_dex::pools::abi_balancer::{self, IBalancerVault};
use arb::venues::bsc_dex::pools::exec_kind::{self, ExecKind};
use arb::venues::bsc_dex::pools::verify::Candidate;
use arb::venues::bsc_dex::pools::{exec_callback, families, load_balancer, verify_balancer};
use arb::venues::bsc_dex::quote::quoter_balancer;

const DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/data/balancer_acsi_2026-11-15/"
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

fn list(v: &Value) -> Vec<U256> {
    v.as_array().unwrap().iter().map(dec).collect()
}

fn pool_of<'a>(probe: &'a Value, a: &str) -> &'a Value {
    probe["pools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["pool"] == a)
        .unwrap()
}

/// Пул из фабрик DEX (isPoolFromFactory у весовой или стейблсвап-фабрики = 1).
fn from_factory(p: &Value) -> bool {
    p["isPoolFromFactory"]["weighted"] == "1" || p["isPoolFromFactory"]["stable"] == "1"
}

/// Состояние по записанным ответам тем же сборщиком, что у чтения на узле (пара — токены i, j).
fn state(p: &Value, i: usize, j: usize) -> VaultPoolData {
    let tokens: Vec<Address> = p["tokens"].as_array().unwrap().iter().map(addr).collect();
    let scaling = list(&p["decimals"])
        .iter()
        .map(|d| U256::from(10u8).pow(U256::from(18u8) - *d))
        .collect();
    let curve = if p["weights"].is_null() {
        let a = list(&p["amp"]);
        assert_eq!(a[2], U256::from(1000u32), "точность A");
        VaultCurve::Stable { amp: a[0] }
    } else {
        VaultCurve::Weighted {
            weights: list(&p["weights"]),
        }
    };
    let id: B256 = p["pool_id"].as_str().unwrap().parse().unwrap();
    let paused = p["paused"][0] == "1";
    let s = load_balancer::state(
        id,
        &tokens,
        list(&p["balances"]),
        scaling,
        dec(&p["swapFee"]),
        paused,
        curve,
        (tokens[i], tokens[j]),
    )
    .expect("состояние");
    let PoolState::VaultPool(d) = s else {
        panic!("не VaultPool")
    };
    assert_eq!((d.i0 as usize, d.i1 as usize), (i, j));
    *d
}

fn neg(v: &Value) -> U256 {
    I256::from_dec_str(v.as_str().unwrap())
        .unwrap()
        .unsigned_abs()
}

#[test]
fn formula_equals_vault_query_given_in() {
    let probe = json("probe.json");
    let mut n = 0;
    for p in probe["pools"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| from_factory(p))
    {
        for q in p["queries"].as_array().unwrap() {
            let (i, j) = (
                q["i"].as_u64().unwrap() as usize,
                q["j"].as_u64().unwrap() as usize,
            );
            let got = balancer_math::amount_out(&state(p, i, j), i, j, dec(&q["amount_in"]));
            let want = q.get("delta_out").map(neg);
            assert_eq!(
                got, want,
                "пул {} {i}→{j} вход {}",
                p["pool"], q["amount_in"]
            );
            n += 1;
        }
    }
    assert_eq!(n, 95);
}

#[test]
fn formula_equals_vault_query_given_out() {
    let probe = json("probe.json");
    let mut n = 0;
    for p in probe["pools"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| from_factory(p))
    {
        for q in p["queries_out"].as_array().into_iter().flatten() {
            let (i, j) = (
                q["i"].as_u64().unwrap() as usize,
                q["j"].as_u64().unwrap() as usize,
            );
            let got = balancer_math::amount_in(&state(p, i, j), i, j, dec(&q["amount_out"]));
            assert_eq!(
                got,
                Some(dec(&q["delta_in"])),
                "пул {} {i}→{j} выход {}",
                p["pool"],
                q["amount_out"]
            );
            n += 1;
        }
    }
    assert_eq!(n, 63);
}

#[test]
fn executor_step_received_equals_our_formula() {
    let probe = json("probe.json");
    let ex = json("exec_sim.json");
    assert_eq!(ex["block"], probe["block"]);
    let mut n = 0;
    for c in ex["exec"].as_array().unwrap() {
        let got = if let Some(route) = c["route"].as_array() {
            let (a, b) = (
                pool_of(&probe, route[0].as_str().unwrap()),
                pool_of(&probe, route[1].as_str().unwrap()),
            );
            let ix = |p: &Value, t: &Value| {
                p["tokens"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .position(|x| x == t)
                    .unwrap()
            };
            let (ai, aj) = (ix(a, &c["token_in"]), ix(a, &c["mid"]));
            let mid = balancer_math::amount_out(&state(a, ai, aj), ai, aj, dec(&c["balance_in"]))
                .unwrap();
            let (bi, bj) = (ix(b, &c["mid"]), ix(b, &c["token_out"]));
            balancer_math::amount_out(&state(b, bi, bj), bi, bj, mid).unwrap()
        } else {
            let p = pool_of(&probe, c["pool"].as_str().unwrap());
            let (i, j) = (
                c["i"].as_u64().unwrap() as usize,
                c["j"].as_u64().unwrap() as usize,
            );
            balancer_math::amount_out(&state(p, i, j), i, j, dec(&c["balance_in"])).unwrap()
        };
        assert_eq!(got, dec(&c["received"]), "{c}");
        n += 1;
    }
    assert_eq!(n, 10);
    assert!(ex["negative"]["received"].is_null());
}

/// Ответы второго прохода проверки по записи: getPool, getPoolTokens, isPoolFromFactory (весовая, стейблсвап).
fn second(p: &Value, reg: Address) -> Vec<Result<Bytes, String>> {
    let spec = dec(&p["getPool"][1]);
    let tokens: Vec<Address> = p["tokens"].as_array().unwrap().iter().map(addr).collect();
    let ret = |b: bool| Ok(Bytes::from(b.abi_encode()));
    vec![
        Ok(Bytes::from((reg, spec).abi_encode_params())),
        Ok(Bytes::from(
            (tokens, list(&p["balances"]), dec(&p["lastChangeBlock"])).abi_encode_params(),
        )),
        ret(p["isPoolFromFactory"]["weighted"] == "1"),
        ret(p["isPoolFromFactory"]["stable"] == "1"),
    ]
}

#[test]
fn verify_takes_factory_pools_and_rejects_impostors() {
    let probe = json("probe.json");
    let fam = families::make("acsi-finance", "balancer_v2").expect("вид");
    let vault: Address = probe["vault"]["address"].as_str().unwrap().parse().unwrap();
    let wf = addr(&probe["factories"]["weighted"]["address"]);
    let sf = addr(&probe["factories"]["stable"]["address"]);
    for p in probe["pools"].as_array().unwrap() {
        let pool = addr(&p["pool"]);
        let t = p["tokens"].as_array().unwrap();
        let cand = Candidate {
            family: fam,
            pool: p["pool"].as_str().unwrap().into(),
            coin: addr(&t[0]),
            quote: addr(&t[1]),
        };
        let plan = (pool, vault, wf, Some(sf));
        let id = verify_balancer::first_pass(
            &plan,
            &Ok(Bytes::from(addr(&p["getVault"]).abi_encode())),
            &Ok(Bytes::from(
                p["pool_id"]
                    .as_str()
                    .unwrap()
                    .parse::<B256>()
                    .unwrap()
                    .abi_encode(),
            )),
        )
        .expect("первый проход");
        assert_eq!(verify_balancer::second_calls(&plan, id).len(), 4);
        let r = verify_balancer::decide(&cand, plan, &second(p, addr(&p["getPool"][0])));
        assert_eq!(
            r.is_ok(),
            from_factory(p),
            "{} {:?}",
            p["pool"],
            r.as_ref().err()
        );
        if !from_factory(p) {
            continue;
        }
        // Самозванец: хранилище под этим id знает другой адрес.
        let other = Address::repeat_byte(0x11);
        assert!(
            verify_balancer::decide(&cand, plan, &second(p, other))
                .unwrap_err()
                .contains("самозванец")
        );
        // Чужое хранилище.
        let bad = (pool, Address::repeat_byte(0x22), wf, Some(sf));
        assert!(
            verify_balancer::first_pass(
                &bad,
                &Ok(Bytes::from(vault.abi_encode())),
                &Ok(Bytes::new())
            )
            .is_err()
        );
        // Пара не из токенов пула.
        let wrong = Candidate {
            coin: Address::repeat_byte(0x33),
            ..cand.clone()
        };
        assert!(verify_balancer::decide(&wrong, plan, &second(p, pool)).is_err());
        // Стейблсвап без dex.balancer_stable_factory — отказ; весовой — принят.
        let no_sf = (pool, vault, wf, None);
        let r = verify_balancer::decide(&cand, no_sf, &second(p, pool)[..3]);
        assert_eq!(
            r.is_ok(),
            p["isPoolFromFactory"]["weighted"] == "1",
            "{}",
            p["pool"]
        );
    }
}

#[test]
fn quoter_decodes_vault_deltas_and_encodes_official_selector() {
    let probe = json("probe.json");
    let p = pool_of(&probe, "0xa22f3e51939488962aa26d42799df75f72816ce0");
    let q = &p["queries"][1];
    let deltas = vec![
        I256::from_dec_str(q["delta_in"].as_str().unwrap()).unwrap(),
        I256::from_dec_str(q["delta_out"].as_str().unwrap()).unwrap(),
    ];
    assert_eq!(
        quoter_balancer::decode(&deltas.abi_encode()),
        Some(neg(&q["delta_out"]))
    );
    let data = quoter_balancer::data(B256::ZERO, Address::ZERO, Address::ZERO, U256::from(1u8));
    let sig = "queryBatchSwap(uint8,(bytes32,uint256,uint256,uint256,bytes)[],address[],(address,bool,address,bool))";
    assert_eq!(&data[..4], &keccak256(sig)[..4]);
    assert_eq!(
        IBalancerVault::queryBatchSwapCall::SELECTOR,
        keccak256(sig)[..4]
    );
}

#[test]
fn kind_step_and_events() {
    let f = families::make("acsi-finance", "balancer_v2").expect("вид");
    assert_eq!(f.kind, PoolKind::VaultPool);
    assert!(f.graphable && !f.tradable && !f.fork);
    assert_eq!(f.executor_kind, Some(ExecKind::Balancer));
    assert_eq!(exec_kind::of("balancer_v2"), Some(ExecKind::Balancer));
    assert!(exec_callback::v2_only(ExecKind::Balancer));
    let id = B256::repeat_byte(0xab);
    let p = Params::Balancer { pool_id: id };
    assert_eq!(p.kind(), 17);
    assert_eq!(B256::abi_decode(&p.data()).unwrap(), id);
    let want: Vec<B256> = [
        "SwapFeePercentageChanged(uint256)",
        "PausedStateChanged(bool)",
        "AmpUpdateStarted(uint256,uint256,uint256,uint256)",
        "AmpUpdateStopped(uint256)",
    ]
    .iter()
    .map(keccak256)
    .collect();
    assert_eq!(abi_balancer::topics(), want);
}
