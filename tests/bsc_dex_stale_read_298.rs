//! Файл: tests/bsc_dex_stale_read_298.rs
//! Что делает: решение 298 (задача t129) на реально записанных данных:
//! - ответ своего узла BSC «−32000 historical state <hash> is not available» (запись 12.11.2026,
//!   tests/data/node_limits_2026-11-12) — StaleData, перечитывание пула по событию НЕ рвёт сессию пачки
//!   (stale_read::is_stale); отказ того же узла по диапазону eth_getLogs (BadResponse) — не StaleData, путь прежний;
//! - очередь повтора пачки (stale_read::requeue): пул встаёт один раз, брошенный снимается из брошенных;
//! - групповое чтение (reload_many::fresh_each): пулы видов, читаемых по одному (thena-v3 algebra_integral — запись
//!   algebra_integral_2026-11-06; metric-bsc metric_omm — запись metric_2026-11-15), — свой свежий блок; пулы ступеней
//!   Multicall3 (PancakeSwap V2/V3, Uniswap V3 — запись node_calls_3_2026-10-24) и пары многомонетного пула
//!   (PoolRef::Multi, nerve — запись nerve_2026-11-15) — в группе на одном блоке, как раньше.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{errors, pools::{stale_read, reload_many, load_batch, families, discover}}.

use alloy_primitives::Address;
use serde_json::Value;

use arb::core::types::PoolRef;
use arb::gateway::errors::ErrorCode;
use arb::venues::bsc_dex::errors::{Op, from_rpc};
use arb::venues::bsc_dex::pools::discover::Found;
use arb::venues::bsc_dex::pools::families::{self, Family};
use arb::venues::bsc_dex::pools::reload::Missing;
use arb::venues::bsc_dex::pools::revive::Abandoned;
use arb::venues::bsc_dex::pools::{load_batch, reload_many, stale_read};

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
