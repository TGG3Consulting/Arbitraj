//! Файл: tests/bsc_dex_multi_pair_exec.rs
//! Что делает: решение 295, тест (г) — запись КАЖДОЙ пары многомонетного пула (`PoolRef::Multi`) даёт верные индексы и
//! токены шага исполнителя и верную котировку — на ЖИВЫХ записях узла BSC:
//! - Nerve 3pool (nerve_2026-11-15) и Ellipsis 3pool (ellipsis_2026-11-15): по каждому записанному calculateSwap(i,j,dx)
//!   / get_dy(i,j,dx) (все 6 направлений × 3 суммы) — запись пары (монета i, монета j), состояние из одного прочитанного
//!   пула (load_saddle / load_curve state_with), наша котировка pool_quote::swap = записанному ответу пула, шаг
//!   executor::entry::params = Params::Saddle / Params::Curve с теми же i, j (данные шага побайтно = кодировке i, j);
//! - Balancer (balancer_acsi_2026-11-15/exec_state.json, 10 записанных сделок): запись пары каждого хопа → шаг
//!   Params::Balancer { pool_id } к хранилищу dex.vault, calldata execute побайтно = записанной, выход цепочки
//!   pool_quote::swap = записанному «получено».
//!
//! Кто вызывает: cargo test --test bsc_dex_multi_pair_exec.
//! От чего зависит: tests/multi_common, arb::venues::bsc_dex::{pools, quote::pool_quote, exec::executor}.

mod multi_common;

use std::str::FromStr;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, U256};
use chrono::Utc;
use serde_json::Value;

use arb::core::types::{Pool, PoolRef, PoolState, VaultCurve, VenueId};
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::exec::executor::abi::ArbExecutor;
use arb::venues::bsc_dex::exec::executor::entry;
use arb::venues::bsc_dex::exec::executor::kind::{self, ALL_BPS, Params, StepSpec};
use arb::venues::bsc_dex::pools::registry::Entry;
use arb::venues::bsc_dex::pools::{families, load_balancer, load_curve, load_saddle};
use arb::venues::bsc_dex::quote::pool_quote;

use multi_common::{bad, ctx, decimals, loader_answers, rec, set_dex, word};

/// Запись пары (a, b) пула `pool` вида `kind` DEX `id` с состоянием `state`. Ошибок нет (паника теста).
fn entry_of(
    id: &str,
    kind: &str,
    pool: Address,
    a: Address,
    b: Address,
    state: PoolState,
) -> Entry {
    let f = families::make(id, kind).unwrap();
    let (t0, t1) = if a < b { (a, b) } else { (b, a) };
    Entry {
        family: f,
        pool: Pool {
            venue: VenueId("bsc_dex".into()),
            family: f.id.into(),
            kind: f.kind,
            reference: PoolRef::Multi {
                contract: pool,
                token0: t0,
                token1: t1,
            },
            token0: t0,
            token1: t1,
            fee_pips: None,
            tick_spacing: None,
            bin_step: None,
            hooks: None,
            state,
            block: 1,
            received_at: Utc::now(),
        },
        ticker: "X".into(),
        coin: a,
        usdt: b,
        key: None,
        ticks: None,
        bins: None,
        synced_block: 1,
        synced_log: None,
        valid: true,
        asleep: false,
        state_gen: 0,
    }
}

/// Монеты пула по ответам «монета k» записи (до отката). Ошибок нет (паника теста).
fn coins_of(probe: &Value, getter: &str) -> Vec<Address> {
    (0..)
        .map_while(|k| multi_common::answer(probe, &format!("{getter}({k})")).ok())
        .map(|b| Address::from_slice(&b[12..32]))
        .collect()
}

/// Сверка всех записанных котировок пула по записям пар. Принимает: окружение, запись, префикс, квотер, состояние пары,
/// ожидаемый шаг по (i, j). Возвращает: число сверенных котировок. Ошибок нет (паника теста).
fn check(
    ctx: &Ctx,
    probe: &Value,
    pre: &str,
    q: &str,
    coins: &[Address],
    mk: &dyn Fn(Address, Address) -> Entry,
    step: &dyn Fn(u8, u8) -> Params,
) -> usize {
    let mut n = 0;
    for c in probe["calls"].as_array().unwrap() {
        let name = c["name"].as_str().unwrap();
        let Some(args) = name.strip_prefix(&format!("{pre} {q}(")) else {
            continue;
        };
        let v: Vec<&str> = args.trim_end_matches(')').split(',').collect();
        let (i, j): (usize, usize) = (v[0].parse().unwrap(), v[1].parse().unwrap());
        let dx = U256::from_str(v[2]).unwrap();
        let e = mk(coins[i], coins[j]);
        let got = pool_quote::swap(ctx, &e, coins[i], true, dx).unwrap();
        assert_eq!(
            got.amount_out,
            word(probe, name),
            "{name}: котировка записи пары"
        );
        let (p, at) = entry::params(ctx, &e, coins[i]).unwrap();
        let want = step(i as u8, j as u8);
        assert_eq!(
            p.data(),
            want.data(),
            "{name}: данные шага — индексы записи"
        );
        assert_eq!(
            at,
            e.pool.reference.address(),
            "{name}: шаг — к контракту пула"
        );
        n += 1;
    }
    n
}

#[tokio::test]
async fn nerve_and_ellipsis_steps_for_each_pair() {
    let c = ctx("exec").await;
    let probe = rec("nerve_2026-11-15", "probe.json");
    let pool = Address::from_str(probe["pool"].as_str().unwrap()).unwrap();
    let (calls, tail) = load_saddle::calls(pool);
    let (head, t) = loader_answers(
        &probe,
        &calls,
        tail,
        ["nerve getToken", "nerve getTokenBalance"],
        &[
            "nerve getAPrecise()",
            "nerve swapStorage()",
            "nerve paused()",
        ],
    );
    let n = head.len();
    let raw = load_saddle::Raw {
        pool,
        got: head.into_iter().chain(t).collect(),
        tail: n,
        decimals: decimals(&probe, "nerve", "getToken", 3),
    };
    let coins = coins_of(&probe, "nerve getToken");
    let mk = |a: Address, b: Address| {
        let s = load_saddle::state_with(&raw, if a < b { (a, b) } else { (b, a) }, &bad).unwrap();
        entry_of("nerve", "saddle_stableswap", pool, a, b, s)
    };
    assert_eq!(
        check(
            &c,
            &probe,
            "nerve",
            "calculateSwap",
            &coins,
            &mk,
            &|i, j| Params::Saddle { i, j }
        ),
        18
    );
    let probe = rec("ellipsis_2026-11-15", "probe.json");
    let pool = Address::from_str(probe["pool"].as_str().unwrap()).unwrap();
    let (calls, tail) = load_curve::calls(pool);
    let (head, t) = loader_answers(
        &probe,
        &calls,
        tail,
        ["ellipsis coins", "ellipsis balances"],
        &["ellipsis A()", "ellipsis fee()"],
    );
    let n = head.len();
    let raw = load_curve::Raw {
        pool,
        got: head.into_iter().chain(t).collect(),
        tail: n,
        decimals: decimals(&probe, "ellipsis", "coins", 3),
    };
    let coins = coins_of(&probe, "ellipsis coins");
    let mk = |a: Address, b: Address| {
        let s = load_curve::state_with(&raw, if a < b { (a, b) } else { (b, a) }, &bad).unwrap();
        entry_of("ellipsis", "curve_stableswap", pool, a, b, s)
    };
    let step = |i: u8, j: u8| Params::Curve {
        i: i as i128,
        j: j as i128,
    };
    assert_eq!(
        check(&c, &probe, "ellipsis", "get_dy", &coins, &mk, &step),
        18
    );
}

#[tokio::test]
async fn balancer_steps_for_each_pair() {
    let st = rec("balancer_acsi_2026-11-15", "exec_state.json");
    let c = ctx("exec-bal").await;
    set_dex(&c, "acsi-finance", "dex.vault", st["vault"].clone()).await;
    let addr = |v: &Value| v.as_str().unwrap().parse::<Address>().unwrap();
    let dec = |v: &Value| U256::from_str_radix(v.as_str().unwrap(), 10).unwrap();
    let list = |v: &Value| v.as_array().unwrap().iter().map(dec).collect::<Vec<U256>>();
    let pools: Vec<&Value> = st["pools"].as_array().unwrap().iter().collect();
    let find = |a: &Value| *pools.iter().find(|p| p["pool"] == *a).unwrap();
    // Запись пары хопа: состояние пары — load_balancer::state по записанному состоянию пула (как bsc_dex_balancer_acsi_live).
    let mk = |p: &Value, a: Address, b: Address| {
        let tokens: Vec<Address> = p["tokens"].as_array().unwrap().iter().map(addr).collect();
        let scaling = list(&p["decimals"])
            .iter()
            .map(|d| U256::from(10u8).pow(U256::from(18u8) - *d))
            .collect();
        let curve = if p["weights"].is_null() {
            VaultCurve::Stable {
                amp: list(&p["amp"])[0],
            }
        } else {
            VaultCurve::Weighted {
                weights: list(&p["weights"]),
            }
        };
        let id: B256 = p["pool_id"].as_str().unwrap().parse().unwrap();
        let pair = if a < b { (a, b) } else { (b, a) };
        let s = load_balancer::state(
            id,
            &tokens,
            list(&p["balances"]),
            scaling,
            dec(&p["swapFee"]),
            p["paused"][0] == "1",
            curve,
            pair,
        )
        .unwrap();
        entry_of("acsi-finance", "balancer_v2", addr(&p["pool"]), a, b, s)
    };
    let mut n = 0;
    for x in st["exec"].as_array().unwrap() {
        let hops: Vec<(&Value, Address, Address)> = match x["route"].as_array() {
            Some(r) => vec![
                (find(&r[0]), addr(&x["token_in"]), addr(&x["mid"])),
                (find(&r[1]), addr(&x["mid"]), addr(&x["token_out"])),
            ],
            None => vec![(
                find(&x["pool"]),
                addr(&x["token_in"]),
                addr(&x["token_out"]),
            )],
        };
        let mut steps = Vec::new();
        let mut amount = dec(&x["balance_in"]);
        for (p, a, b) in &hops {
            let e = mk(p, *a, *b);
            let (params, at) = entry::params(&c, &e, *a).unwrap();
            assert!(matches!(params, Params::Balancer { .. }));
            steps.push(
                kind::encode(&StepSpec {
                    params,
                    pool: at,
                    token_in: *a,
                    token_out: *b,
                    share_bps: ALL_BPS,
                })
                .unwrap(),
            );
            amount = pool_quote::swap(&c, &e, *a, true, amount)
                .unwrap()
                .amount_out;
        }
        let call = ArbExecutor::executeCall {
            tokenIn: addr(&x["token_in"]),
            amountIn: U256::ZERO,
            tokenOut: addr(&x["token_out"]),
            minOut: U256::ZERO,
            deadline: U256::from(4_102_444_800u64),
            steps,
        };
        let mut want =
            alloy_primitives::hex::decode(x["calldata_selector"].as_str().unwrap()).unwrap();
        for w in x["calldata_words"].as_array().unwrap() {
            want.extend_from_slice(&dec(w).to_be_bytes::<32>());
        }
        assert_eq!(call.abi_encode(), want, "{}", x["pool"]);
        assert_eq!(amount, dec(&x["received"]), "{x}");
        n += 1;
    }
    assert_eq!(n, 10);
}
