//! Файл: tests/bsc_dex_stale_read_298.rs
//! Что делает: решение 298 (задача t129) на реально записанных данных:
//! - ответ своего узла BSC «−32000 historical state <hash> is not available» (запись 12.11.2026,
//!   tests/data/node_limits_2026-11-12) — StaleData, перечитывание пула по событию НЕ рвёт сессию пачки
//!   (stale_read::is_stale); отказ того же узла по диапазону eth_getLogs (BadResponse) — не StaleData, путь прежний;
//! - очередь повтора пачки (stale_read::requeue): пул встаёт один раз, брошенный снимается из брошенных;
//! - групповое чтение (reload_many::fresh_each): пулы видов, читаемых по одному (thena-v3 algebra_integral — запись
//!   algebra_integral_2026-11-06; metric-bsc metric_omm — запись metric_2026-11-15), — свой свежий блок; пулы ступеней
//!   Multicall3 (PancakeSwap V2/V3, Uniswap V3 — запись node_calls_3_2026-10-24) и пары многомонетного пула
//!   (PoolRef::Multi, nerve — запись nerve_2026-11-15) — в группе на одном блоке, как раньше;
//! - ревью 298 на парах Nerve 3pool (PoolRef::Multi, состояние из записи nerve_2026-11-15, блок записи 133724547):
//!   пара в очереди повтора — позиция лога отмечается (stale_read::defer), раньше учтённая не откатывается; блок
//!   повтора не ниже synced_block записей (reload_many::floor_block) — событие новее головы HTTP не теряется.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{errors, pools::{stale_read, reload_many, load_batch, families, discover}}.

mod multi_common;

use alloy_primitives::Address;
use chrono::Utc;
use serde_json::Value;

use arb::core::types::{Pool, PoolRef, VenueId};
use arb::gateway::errors::ErrorCode;
use arb::venues::bsc_dex::errors::{Op, from_rpc};
use arb::venues::bsc_dex::pools::discover::Found;
use arb::venues::bsc_dex::pools::families::{self, Family};
use arb::venues::bsc_dex::pools::registry::{Entry, Registry};
use arb::venues::bsc_dex::pools::reload::Missing;
use arb::venues::bsc_dex::pools::revive::Abandoned;
use arb::venues::bsc_dex::pools::{load_batch, load_saddle, reload_many, stale_read};

use multi_common::{PAIRS3, bad, decimals, loader_answers, pair};

fn rec(path: &str) -> Value {
    let p = format!("{}/tests/data/{path}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(p).expect("запись")).expect("JSON")
}

/// Адрес из 32-байтового слова ответа eth_call (последние 20 байт).
fn word_addr(raw: &str) -> Address {
    raw[raw.len() - 40..].parse().unwrap()
}

/// Найденный пул из записанных адресов (поля, не нужные проверке, — пусто).
fn found(family: &'static Family, reference: PoolRef, token0: Address, token1: Address) -> Found {
    Found {
        family,
        reference,
        token0,
        token1,
        coin: token0,
        fee_pips: None,
        tick_spacing: None,
        bin_step: None,
        key: None,
        hooks: None,
    }
}

#[test]
fn stale_state_answer_does_not_break_session() {
    let v = rec("node_limits_2026-11-12/responses.json");
    let err = |x: &Value| {
        let e = &x["error"];
        from_rpc(
            e["code"].as_i64().unwrap(),
            e["message"].as_str().unwrap(),
            "узел BSC (HTTP)",
            Op::Read,
            &e.to_string(),
        )
    };
    let old = v["state"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["ok"] == false)
        .unwrap();
    let e = err(old);
    assert_eq!(e.code, ErrorCode::StaleData);
    assert!(e.message.contains("is not available"));
    assert!(stale_read::is_stale(&e));
    // Прочая ошибка узла (диапазон eth_getLogs) — не StaleData: путь прежний (конец сессии пачки).
    let range = v["getLogs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["ok"] == false)
        .unwrap();
    assert!(!stale_read::is_stale(&err(range)));
}

#[test]
fn requeue_once_and_leaves_abandoned() {
    let mut missing = vec![Missing { i: 3, tries: 2 }];
    let mut abandoned = vec![
        Abandoned {
            i: 5,
            setting: None,
        },
        Abandoned {
            i: 7,
            setting: Some("dex.integral_tick_steps".into()),
        },
    ];
    // Уже ждёт — без второй записи, счёт попыток не сбрасывается.
    stale_read::requeue(&mut missing, &mut abandoned, 3);
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0].tries, 2);
    // Брошенный — снят из брошенных, ждёт с нуля.
    stale_read::requeue(&mut missing, &mut abandoned, 5);
    assert_eq!(
        missing.iter().map(|m| (m.i, m.tries)).collect::<Vec<_>>(),
        vec![(3, 2), (5, 0)]
    );
    assert_eq!(abandoned.iter().map(|a| a.i).collect::<Vec<_>>(), vec![7]);
}

#[test]
fn fresh_block_only_for_one_by_one_kinds() {
    // thena-v3 (Algebra Integral) — живой случай 15.11: пул читается по одному, 4–5 + 2·шагов вызовов.
    let ai = rec("algebra_integral_2026-11-06/probe.json");
    let p0 = &ai["pools"][0];
    assert_eq!(p0["dex_id"], "thena-v3");
    let thena = families::make("thena-v3", "algebra_integral").unwrap();
    let f = found(
        thena,
        PoolRef::Contract(p0["pool"].as_str().unwrap().parse().unwrap()),
        word_addr(p0["calls"]["token0"]["raw"].as_str().unwrap()),
        word_addr(p0["calls"]["token1"]["raw"].as_str().unwrap()),
    );
    assert!(load_batch::one_by_one(thena));
    assert!(reload_many::fresh_each(&f));
    // metric-bsc (Metric OMM) — тоже по одному.
    let m = rec("metric_2026-11-15/probe.json");
    let mp = &m["pools"][0];
    let imm = mp["immutables"].as_array().unwrap();
    let metric = families::make("metric-bsc", "metric_omm").unwrap();
    let a = |k: usize| {
        format!("{:0>40}", imm[k].as_str().unwrap())
            .parse::<Address>()
            .unwrap()
    };
    let f = found(
        metric,
        PoolRef::Contract(mp["pool"].as_str().unwrap().parse().unwrap()),
        a(1),
        a(2),
    );
    assert!(reload_many::fresh_each(&f));
    // Пулы ступеней Multicall3 — в группе на одном блоке (согласованность прежняя).
    let calls = rec("node_calls_3_2026-10-24.json");
    for p in calls["pools"].as_array().unwrap() {
        let kind = match p["kind"].as_str().unwrap() {
            "v2" => "pancakeswap_v2",
            "pcs_v3" => "pancakeswap_v3",
            "uni_v3" => "uniswap_v3",
            _ => continue,
        };
        let dex = p["tag"]
            .as_str()
            .unwrap()
            .split(' ')
            .nth(1)
            .unwrap()
            .to_string();
        let fam = families::make(&dex, kind).unwrap();
        let addr: Address = p["addr"].as_str().unwrap().parse().unwrap();
        assert!(!load_batch::one_by_one(fam), "{dex} {kind}");
        assert!(!reload_many::fresh_each(&found(
            fam,
            PoolRef::Contract(addr),
            addr,
            addr
        )));
    }
    // Пара многомонетного пула (решение 295): вид «по одному», но пары контракта читаются одним чтением — в группе.
    let nerve = rec("nerve_2026-11-15/probe.json");
    let pool: Address = nerve["pool"].as_str().unwrap().parse().unwrap();
    let coins = rec("nerve_2026-11-15/coins.json");
    let c = |sym: &str| -> Address { coins[sym].as_str().unwrap().parse().unwrap() };
    let (x, y) = (c("BUSD"), c("USDT"));
    let (t0, t1) = if x < y { (x, y) } else { (y, x) };
    let saddle = families::make("nerve", "saddle_stableswap").unwrap();
    assert!(load_batch::one_by_one(saddle));
    let multi = found(
        saddle,
        PoolRef::Multi {
            contract: pool,
            token0: t0,
            token1: t1,
        },
        t0,
        t1,
    );
    assert!(!reload_many::fresh_each(&multi));
}

/// Пары Nerve 3pool из записи: (найденный пул, запись реестра с отметкой synced_block = блок записи).
fn nerve_pairs() -> Vec<(Found, Entry)> {
    let probe = multi_common::rec("nerve_2026-11-15", "probe.json");
    let pool: Address = probe["pool"].as_str().unwrap().parse().unwrap();
    let at = probe["block_dec"].as_u64().unwrap();
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
    let dec = decimals(&probe, "nerve", "getToken", 3);
    let n = head.len();
    let raw = load_saddle::Raw {
        pool,
        got: head.iter().cloned().chain(t.iter().cloned()).collect(),
        tail: n,
        decimals: dec,
    };
    let fam = families::make("nerve", "saddle_stableswap").unwrap();
    PAIRS3
        .iter()
        .map(|(a, b)| {
            let (t0, t1) = pair("nerve_2026-11-15", a, b);
            let reference = PoolRef::Multi {
                contract: pool,
                token0: t0,
                token1: t1,
            };
            let e = Entry {
                family: fam,
                pool: Pool {
                    venue: VenueId("bsc_dex".into()),
                    family: fam.id.into(),
                    kind: fam.kind,
                    reference: reference.clone(),
                    token0: t0,
                    token1: t1,
                    fee_pips: None,
                    tick_spacing: None,
                    bin_step: None,
                    hooks: None,
                    state: load_saddle::state_with(&raw, (t0, t1), &bad).unwrap(),
                    block: at,
                    received_at: Utc::now(),
                },
                ticker: (*a).into(),
                coin: t0,
                usdt: t1,
                key: None,
                ticks: None,
                bins: None,
                synced_block: at,
                synced_log: None,
                valid: false,
                asleep: false,
                state_gen: 0,
            };
            (found(fam, reference, t0, t1), e)
        })
        .collect()
}

#[test]
fn multi_pair_waiting_defers_and_floor_keeps_events() {
    let pairs = nerve_pairs();
    let at = pairs[0].1.synced_block;
    let reg = Registry::default();
    for (_, e) in &pairs {
        reg.put(e.clone());
    }
    // Пара 0 ждёт повтора (индекс в пачке 0), пары 1–2 — нет.
    let missing = vec![Missing { i: 0, tries: 0 }];
    assert!(stale_read::waiting(&missing, 0));
    assert!(!stale_read::waiting(&missing, 1));
    // Лог пула на блоке новее записи — у ждущей пары позиция отмечается, узел не спрашивается.
    let r0 = &pairs[0].0.reference;
    stale_read::defer(&reg, r0, at + 3, 7);
    let e0 = reg.get(r0).unwrap();
    assert_eq!((e0.synced_block, e0.synced_log), (at + 3, Some(7)));
    assert!(e0.has_applied(at + 3, 7) && !e0.has_applied(at + 3, 8));
    assert!(!e0.valid, "отметка позиции не делает пару достоверной");
    // Более ранний лог отметку не откатывает.
    stale_read::defer(&reg, r0, at + 1, 99);
    assert_eq!(reg.get(r0).unwrap().synced_block, at + 3);
    // Блок повтора: не ниже synced_block записей (голова HTTP, например, ещё на блоке записи).
    let items: Vec<(&Found, &str)> = pairs.iter().map(|(f, _)| (f, "")).collect();
    assert_eq!(reload_many::floor_block(&reg, &items, at), at + 3);
    assert_eq!(reload_many::floor_block(&reg, &items[1..], at), at);
    assert_eq!(
        reload_many::floor_block(&reg, &items[1..], at + 10),
        at + 10
    );
    // Пула нет в реестре — только наименьший блок.
    assert_eq!(
        reload_many::floor_block(&Registry::default(), &items, at),
        at
    );
}
