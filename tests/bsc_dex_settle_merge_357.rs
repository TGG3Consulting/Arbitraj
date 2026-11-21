//! Файл: tests/bsc_dex_settle_merge_357.rs
//! Что делает: ревью слияния «357 × 339» — правило достоверности пулов с отметками (pools/settle.rs, settle_head.rs,
//! settle_close.rs, registry.rs) на РЕАЛЬНЫХ записях tests/data/lb_v21_2026-11-15 через локальный узел на записи
//! (tests/lb_node): пул BTC.b/WBNB 0x717d…073d, прочитанный настоящим загрузчиком на probe.json (133716961), настоящий лог
//! Swap swap_log.json (133727176, бин 8399652), состояние после него — probe_time.json (133732452; других логов пула
//! между блоками нет, README записи).
//! - Б1: голова не прочиталась (head_failed) → «нужно полное чтение»; событие на паузе 339 применяется БЕЗ перечитывания
//!   бинов (как listen_log.rs `Bins if waiting`) → голова (lb_head::renewed) достоверность не возвращает; полное чтение
//!   (load_lb::load на probe_time → load::renewed → registry put) снимает отметку, бин события в памяти — записанный.
//! - Б2: отметка «в обновлении» снимается только логом того же действия (settle.rs lost): Active — тот же блок, Follow —
//!   та же транзакция; лог другого блока — «потерян» (полное чтение); invalidate снимает отметки пачки (forget_all).
//! - Н3: закрытие на голове — только когда логам пула негде ждать (settle_close.rs in_transit / due).
//! - Н4: retain реестра снимает отметки удалённых пулов.
//!
//! Без теста (нет записи): Follow на настоящем SwapFee Integral 1.2.10 (у пулов бота его нет — запись 17.11);
//! вывод из НЕАКТИВНОГО бина, при котором голова «равны» (в записях только обмен в активном бине — голова там «не
//! равны»; правило «голова не поднимает» проверено на «равны» у USDC/USDT — tests/bsc_dex_settle_357.rs).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/native_common, tests/lb_node, arb::venues::bsc_dex::pools::{settle, settle_close, settle_head,
//! lb_head, events, load, load_lb, registry}.
//! Настройки теста: узел (lb_node::attach), dex.bin_depth_usd = 3 [bsc_dex].

mod lb_node;
mod native_common;

use alloy::rpc::types::Log;
use alloy_primitives::{Address, B256, Bytes, LogData};
use serde_json::json;

use arb::core::types::{PoolRef, PoolState};
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::node::call::{block_tag, try_many};
use arb::venues::bsc_dex::pools::events::{self, Applied};
use arb::venues::bsc_dex::pools::load::{self, Loaded};
use arb::venues::bsc_dex::pools::registry::{Entry, Registry};
use arb::venues::bsc_dex::pools::settle::Need;
use arb::venues::bsc_dex::pools::{lb_head, load_lb, settle_close};
use arb::venues::bsc_dex::tokens::Tokens;
use lb_node::{Rec, addr};

const BTCB_WBNB: &str = "0x717d06b4d8bc6b71d5a2c45d8cc417930aec073d";
const BIN: u32 = 8_399_652;

/// Узел на записи (probe, probe_time), окружение, токены с decimals записи.
async fn scene(tag: &str) -> (Ctx, Rec, Rec, Tokens) {
    let c = native_common::ctx(tag).await;
    let url = lb_node::node(vec![
        lb_node::rec("probe.json"),
        lb_node::rec("probe_time.json"),
    ])
    .await;
    lb_node::attach(&c, &url).await;
    let v = Scope::Venue("bsc_dex".into());
    c.settings
        .set("dex.bin_depth_usd", v, Some(json!(3)), "тест")
        .await
        .unwrap();
    let probe = lb_node::rec("probe.json");
    let t = Tokens::default();
    for p in probe.pools.values() {
        let d = |k: &str| p[k].as_str().unwrap().parse::<u8>().unwrap();
        t.remember_decimals(addr(&p["tokenX"]), d("decimalsX"));
        t.remember_decimals(addr(&p["tokenY"]), d("decimalsY"));
    }
    (c, probe, lb_node::rec("probe_time.json"), t)
}

/// Пул, прочитанный настоящим загрузчиком на блоке записи `r` (как tests/bsc_dex_settle_357.rs).
async fn loaded(c: &Ctx, t: &Tokens, r: &Rec) -> Entry {
    let a: Address = BTCB_WBNB.parse().unwrap();
    let p = &r.pools[&a];
    let (x, y) = (addr(&p["tokenX"]), addr(&p["tokenY"]));
    let pair = (x.min(y), x.max(y));
    let (state, bins) = load_lb::load(c, t, a, pair, y, r.block).await.unwrap();
    let mut e = native_common::entry("lb_v21", None);
    e.pool.reference = PoolRef::Contract(a);
    e.pool.token0 = pair.0;
    e.pool.token1 = pair.1;
    e.pool.bin_step = Some(p["binStep"].as_str().unwrap().parse().unwrap());
    e.pool.state = state;
    e.pool.block = r.block;
    e.bins = Some(bins);
    e.usdt = y;
    e.coin = x;
    e.synced_block = r.block;
    e
}

/// Настоящий лог Swap (swap_log.json) как лог подписки.
fn swap_log() -> Log {
    let l = &lb_node::json_of("swap_log.json")["logs"][0];
    let topics: Vec<B256> = l["topics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t.as_str().unwrap().parse().unwrap())
        .collect();
    let raw = alloy_primitives::hex::decode(l["data"].as_str().unwrap()).unwrap();
    let index = l["logIndex"].as_str().unwrap().trim_start_matches("0x");
    Log {
        inner: alloy_primitives::Log {
            address: addr(&l["address"]),
            data: LogData::new(topics, Bytes::from(raw)).unwrap(),
        },
        block_number: Some(l["blockNumber"].as_u64().unwrap()),
        log_index: Some(u64::from_str_radix(index, 16).unwrap()),
        ..Default::default()
    }
}

/// Б1: голова не прочиталась → пауза (событие без перечитывания бинов) → голова достоверность не возвращает → полное
/// чтение возвращает, и бин события в памяти — настоящий.
#[tokio::test]
async fn head_fail_then_pause_only_full_read_restores() {
    let (c, probe, time, t) = scene("settle-merge-b1").await;
    let mem = loaded(&c, &t, &probe).await;
    let r = mem.pool.reference.clone();
    let before = mem.bins.as_ref().unwrap().reserves.get(&BIN).copied();
    let reg = Registry::default();
    reg.put(mem);
    // Голова не прочиталась (per_block_lb.rs): недостоверен, «нужно полное чтение».
    reg.holds().head_failed(&reg, &r);
    assert!(!reg.get(&r).unwrap().valid);
    assert!(reg.holds().needs_full(&r));
    // Пауза 339: событие пула применяется без перечитывания бинов (listen_log.rs `Bins if waiting => Ok(())`).
    let log = swap_log();
    let a = reg.update(&r, |e| events::apply(e, &log)).unwrap();
    assert_eq!(a, Applied::Bins(vec![BIN]));
    let stale = reg.get(&r).unwrap();
    assert_eq!(
        stale.bins.as_ref().unwrap().reserves.get(&BIN).copied(),
        before
    );
    // Пауза прошла: голова (тем же путём, что per_block_lb.rs) — достоверность не возвращает в любом исходе сравнения.
    let a_addr = r.address();
    let calls = lb_head::calls(a_addr, lb_head::active_of(&stale).unwrap());
    let got = try_many(&c, &calls, &block_tag(time.block)).await.unwrap();
    let ans: Vec<Bytes> = got.into_iter().map(|x| x.unwrap()).collect();
    let PoolState::LbBin(dmem) = &stale.pool.state else {
        panic!("не LB");
    };
    let (d, _bin) = lb_head::decode(&ans, dmem, time.ts).unwrap();
    let head = lb_head::renewed(&stale, time.block, d);
    assert!(!head.valid, "голова не поднимает достоверность");
    reg.put(head);
    assert!(
        reg.holds().needs_full(&r),
        "отметку снимает только полное чтение"
    );
    // Полное чтение (per_block_lb.rs при needs_full → reload::load_one_block → load::refresh → registry put).
    let p = &time.pools[&a_addr];
    let (x, y) = (addr(&p["tokenX"]), addr(&p["tokenY"]));
    let (state, bins) = load_lb::load(&c, &t, a_addr, (x.min(y), x.max(y)), y, time.block)
        .await
        .unwrap();
    let full = load::renewed(
        &reg.get(&r).unwrap(),
        time.block,
        Loaded {
            state,
            ticks: None,
            bins: Some(bins),
            fee_pips: None,
        },
    );
    reg.put(full);
    let e = reg.get(&r).unwrap();
    assert!(e.valid);
    assert!(reg.holds().get(&r).is_none(), "полное чтение сняло отметку");
    let want = lb_node::bins_of(&time.pools[&a_addr])[&BIN];
    assert_eq!(e.bins.as_ref().unwrap().reserves.get(&BIN), Some(&want));
    assert_ne!(
        Some(want),
        before,
        "бин события изменился — без полного чтения в памяти остался бы старый"
    );
}

/// Б2 и Н4: «в обновлении» снимает только лог того же действия; обрыв (invalidate) и retain снимают отметки.
#[tokio::test]
async fn hold_released_only_by_same_action() {
    let (c, probe, time, t) = scene("settle-merge-b2").await;
    let mem = loaded(&c, &t, &probe).await;
    let r = mem.pool.reference.clone();
    let reg = Registry::default();
    reg.put(mem);
    let log = swap_log();
    let (n, tx) = (log.block_number.unwrap(), log.transaction_index);
    // Active: лог того же блока — своё действие; другого блока (probe_time — блок той же пары) — логи потеряны.
    reg.holds()
        .hold(&reg, &r, (n, Need::Active(BIN)), Vec::new(), false);
    assert!(!reg.get(&r).unwrap().valid);
    assert!(!reg.holds().lost(&r, n, tx));
    assert!(reg.holds().lost(&r, time.block, tx));
    // Follow открыт без транзакции (step ставит её по открывшему логу): лог другой транзакции того же блока — потерян.
    reg.holds()
        .hold(&reg, &r, (n, Need::Follow), Vec::new(), false);
    assert!(reg.holds().lost(&r, n, Some(0)));
    // Обрыв сессии (listen_log::invalidate): отметки пачки — долой, достоверность не возвращается.
    reg.holds().forget_all([&r]);
    assert!(reg.holds().get(&r).is_none());
    assert!(!reg.get(&r).unwrap().valid);
    // Н4: retain реестра снимает отметки удалённых пулов.
    reg.holds()
        .hold(&reg, &r, (n, Need::Active(BIN)), Vec::new(), false);
    assert!(reg.holds().any());
    reg.retain(|_| false);
    assert!(
        !reg.holds().any(),
        "отметка удалённого пула не держит ветку голов"
    );
}

/// Н3: закрытие на голове — только когда логам пула негде ждать.
#[test]
fn close_only_when_nothing_in_transit() {
    let probe = lb_node::rec("probe.json");
    let time = lb_node::rec("probe_time.json");
    let (b, later) = (probe.block, time.block);
    // В полосе пула есть события — в пути.
    assert!(settle_close::in_transit(1, 0, later, b));
    // Канал пачки не пуст, а сессия ещё не принимала лог блока позже отметки — может быть в пути.
    assert!(settle_close::in_transit(0, 5, b, b));
    // Канал не пуст, но принят лог более позднего блока — логов блока отметки в канале нет (поток упорядочен).
    assert!(!settle_close::in_transit(0, 5, later, b));
    // Пусто везде.
    assert!(!settle_close::in_transit(0, 0, 0, b));
}
