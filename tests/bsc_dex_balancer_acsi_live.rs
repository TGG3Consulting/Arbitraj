//! Файл: tests/bsc_dex_balancer_acsi_live.rs
//! Что проверяет: решение 289 — Balancer V2 (balancer_v2, ACSI Finance BSC) на РЕАЛЬНЫХ ответах узла BSC, записанных на
//! одном блоке 133699740 (tests/data/balancer_acsi_2026-11-15): хранилище 0xa82f…ceced, пять весовых пулов (3 и 4 токена
//! MINIMAL_SWAP_INFO, 2 токена TWO_TOKEN), три стейблсвап-пула (GENERAL) и пул 0xb978…59ee не из фабрик.
//! 1. Наша математика (load_balancer::state → math/balancer_math.rs) на записанном состоянии даёт ТОТ ЖЕ выход, что
//!    Vault.queryBatchSwap GIVEN_IN, во всех ответах пулов фабрик (включая откаты: вход 35 % баланса весового — BAL#304,
//!    у стейблсвапа 0xe901…c6c5 — несходимость BAL#322 → None), и тот же вход, что GIVEN_OUT.
//! 2. Проверка пула (verify_balancer): пулы фабрик принимаются; пул не из фабрик (0xb978…), настоящий пул другой DEX
//!    (PancakeSwap V2: getVault откатывается, exec_state.json), монета не из токенов пула (USDT у 0x44a9…), стейблсвап без
//!    dex.balancer_stable_factory — отказ; всё — на записанных ответах без подмены.
//! 3. Шаг исполнителя на цепи: выход шага 17 исполнителя v2 (eth_call с подменой кода) = наша формула на состоянии того
//!    же блока — exec_sim.json (блок 133699740, код коммита 9bd0402a) и exec_state.json (блок 133711501, код ревью 289,
//!    состояние пулов и calldata того же вызова): 9 одиночных шагов и маршрут из двух шагов; наш кодировщик шага
//!    (Params::Balancer → kind::encode → execute) даёт побайтно записанную calldata.
//! 4. Стейблсвап 0xe901…: наш invariant() на записанных балансах не сходится (как BAL#322 хранилища); таблица LogExpMath
//!    = строкам исходника (source_excerpt.txt); разбор queryBatchSwap — quoter_balancer.
//! 5. Вид BALANCER = 17, data = poolId, только v2; вид DEX — graphable, шаг BALANCER; topic0 событий пула — по подписям.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{math::{balancer_math, balancer_stable, balancer_log_exp_table}, pools::{load_balancer,
//! verify_balancer, families, exec_kind, exec_callback, abi_balancer}, quote::quoter_balancer, exec::executor::{kind, abi}}.

use alloy::sol_types::{SolCall, SolValue};
use alloy_primitives::{Address, B256, Bytes, I256, U256, keccak256};
use serde_json::Value;

use arb::core::types::{PoolKind, PoolState, VaultCurve, VaultPoolData};
use arb::venues::bsc_dex::exec::executor::abi::ArbExecutor;
use arb::venues::bsc_dex::exec::executor::kind::{self, ALL_BPS, Params, StepSpec};
use arb::venues::bsc_dex::math::{balancer_log_exp_table, balancer_math, balancer_stable};
use arb::venues::bsc_dex::pools::abi_balancer::{self, IBalancerVault};
use arb::venues::bsc_dex::pools::exec_kind::{self, ExecKind};
use arb::venues::bsc_dex::pools::load_balancer_fixed::Fixed;
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
fn verify_takes_factory_pools_and_rejects_others() {
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
fn verify_rejects_real_foreign_pool_and_foreign_coin() {
    let probe = json("probe.json");
    let st = json("exec_state.json");
    let fam = families::make("acsi-finance", "balancer_v2").expect("вид");
    let vault: Address = probe["vault"]["address"].as_str().unwrap().parse().unwrap();
    let wf = addr(&probe["factories"]["weighted"]["address"]);
    let sf = addr(&probe["factories"]["stable"]["address"]);
    // Настоящий пул другой DEX (PancakeSwap V2): getVault() и getPoolId() откатываются — первый проход отклоняет.
    let f = &st["foreign"];
    let pc = addr(&f["pool"]);
    let rev = |v: &Value| Err::<Bytes, String>(v["revert"].as_str().unwrap().to_string());
    let r = verify_balancer::first_pass(
        &(pc, vault, wf, Some(sf)),
        &rev(&f["getVault"]),
        &rev(&f["getPoolId"]),
    );
    assert!(r.unwrap_err().contains("getVault"));
    // Пул фабрики 0x44a9… с монетой USDT, которой среди его токенов нет (ответы — записанные, без подмены).
    let p = pool_of(&probe, "0x44a9ce69ef2a71a9630697ca5cab3f4adaf8f90d");
    let usdt: Address = "0x55d398326f99059ff775485246999027b3197955"
        .parse()
        .unwrap();
    assert!(
        !p["tokens"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| addr(t) == usdt)
    );
    let pool = addr(&p["pool"]);
    let cand = Candidate {
        family: fam,
        pool: p["pool"].as_str().unwrap().into(),
        coin: usdt,
        quote: addr(&p["tokens"][2]),
    };
    let r = verify_balancer::decide(
        &cand,
        (pool, vault, wf, Some(sf)),
        &second(p, addr(&p["getPool"][0])),
    );
    assert!(r.unwrap_err().contains("нет пары"));
}

#[test]
fn exec_state_step_calldata_and_received() {
    let st = json("exec_state.json");
    let vault = addr(&st["vault"]);
    let pools: Vec<&Value> = st["pools"].as_array().unwrap().iter().collect();
    let find = |a: &Value| *pools.iter().find(|p| p["pool"] == *a).unwrap();
    let ix = |p: &Value, t: &Value| {
        p["tokens"]
            .as_array()
            .unwrap()
            .iter()
            .position(|x| x == t)
            .unwrap()
    };
    let id = |p: &Value| p["pool_id"].as_str().unwrap().parse::<B256>().unwrap();
    assert_eq!(st["vault_paused"][0], "0");
    let mut n = 0;
    for c in st["exec"].as_array().unwrap() {
        let hops: Vec<(&Value, Value, Value)> = match c["route"].as_array() {
            Some(r) => vec![
                (find(&r[0]), c["token_in"].clone(), c["mid"].clone()),
                (find(&r[1]), c["mid"].clone(), c["token_out"].clone()),
            ],
            None => vec![(
                find(&c["pool"]),
                c["token_in"].clone(),
                c["token_out"].clone(),
            )],
        };
        // Наш кодировщик шага (Params::Balancer — как executor/entry.rs) → calldata execute побайтно = записанной.
        let steps: Vec<_> = hops
            .iter()
            .map(|(p, a, b)| {
                kind::encode(&StepSpec {
                    params: Params::Balancer { pool_id: id(p) },
                    pool: vault,
                    token_in: addr(a),
                    token_out: addr(b),
                    share_bps: ALL_BPS,
                })
                .unwrap()
            })
            .collect();
        let call = ArbExecutor::executeCall {
            tokenIn: addr(&c["token_in"]),
            amountIn: U256::ZERO,
            tokenOut: addr(&c["token_out"]),
            minOut: U256::ZERO,
            deadline: U256::from(4_102_444_800u64),
            steps,
        };
        let mut want =
            alloy_primitives::hex::decode(c["calldata_selector"].as_str().unwrap()).unwrap();
        for w in c["calldata_words"].as_array().unwrap() {
            want.extend_from_slice(&dec(w).to_be_bytes::<32>());
        }
        assert_eq!(call.abi_encode(), want, "{}", c["pool"]);
        // Получено шагом 17 = наша формула на состоянии того же блока.
        let mut amount = dec(&c["balance_in"]);
        for (p, a, b) in &hops {
            let (i, j) = (ix(p, a), ix(p, b));
            amount = balancer_math::amount_out(&state(p, i, j), i, j, amount).unwrap();
        }
        assert_eq!(amount, dec(&c["received"]), "{c}");
        n += 1;
    }
    assert_eq!(n, 10);
}

#[test]
fn stable_0xe901_invariant_does_not_converge() {
    let probe = json("probe.json");
    let p = pool_of(&probe, "0xe901c4e9a56a175cb5c94bfb5f821ebd5fc826c5");
    let d = state(p, 0, 1);
    let up: Vec<U256> = d
        .balances
        .iter()
        .zip(&d.scaling)
        .map(|(b, s)| *b * *s)
        .collect();
    let VaultCurve::Stable { amp } = d.curve else {
        panic!("не стейблсвап")
    };
    assert_eq!(balancer_stable::invariant(amp, &up, true), None);
}

#[test]
fn log_exp_table_equals_source() {
    let src = std::fs::read_to_string(format!("{DIR}source_excerpt.txt")).unwrap();
    let num = |name: &str| -> I256 {
        let line = src
            .lines()
            .find(|l| l.contains(&format!("int256 constant {name} = ")))
            .unwrap();
        let v = line.split(" = ").nth(1).unwrap().split(';').next().unwrap();
        I256::from_dec_str(v).unwrap()
    };
    let t = balancer_log_exp_table::table();
    for (k, (x, a)) in t.iter().enumerate() {
        assert_eq!(
            (*x, *a),
            (num(&format!("x{k}")), num(&format!("a{k}"))),
            "x{k}/a{k}"
        );
    }
}

#[test]
fn decode_batch_answers_with_vault_pause() {
    // Ответы пачки чтения (load_balancer::calls) — записанные значения exec_state.json, ABI-кодированные.
    let st = json("exec_state.json");
    for p in st["pools"].as_array().unwrap() {
        let tokens: Vec<Address> = p["tokens"].as_array().unwrap().iter().map(addr).collect();
        let weighted = !p["weights"].is_null();
        let scaling = tokens
            .iter()
            .zip(list(&p["decimals"]))
            .map(|(t, d)| (*t, U256::from(10u8).pow(U256::from(18u8) - d)))
            .collect();
        let fx = Fixed {
            id: p["pool_id"].as_str().unwrap().parse().unwrap(),
            weighted,
            scaling,
            // Решение 450: пулы ACSI — библиотеки v1 (20210418 / 20210624).
            v2: false,
            // Решение 473: dex.balancer_pool не задана — весовой / стейблсвап.
            more: None,
        };
        let w3 =
            |v: &Value| (dec(&v[0]) == U256::from(1u8), dec(&v[1]), dec(&v[2])).abi_encode_params();
        let curve = if weighted {
            list(&p["weights"]).abi_encode()
        } else {
            let a = list(&p["amp"]);
            (a[0], a[1] == U256::from(1u8), a[2]).abi_encode_params()
        };
        let got: Vec<Result<Bytes, String>> = vec![
            Ok((tokens.clone(), list(&p["balances"]), U256::ZERO)
                .abi_encode_params()
                .into()),
            Ok(dec(&p["swapFee"]).abi_encode().into()),
            Ok(w3(&p["paused"]).into()),
            Ok(curve.into()),
            Ok(w3(&st["vault_paused"]).into()),
        ];
        assert_eq!(got.len(), load_balancer::PER_POOL);
        let PoolState::VaultPool(d) =
            load_balancer::decode(&fx, &got, (tokens[0], tokens[1])).unwrap()
        else {
            panic!("не VaultPool")
        };
        assert_eq!(*d, state(p, 0, 1));
        assert!(!d.paused, "{}", p["pool"]);
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
