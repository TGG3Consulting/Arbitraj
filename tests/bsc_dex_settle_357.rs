//! Файл: tests/bsc_dex_settle_357.rs
//! Что делает: решение 357 (задача t154) на РЕАЛЬНЫХ записях tests/data/lb_v21_2026-11-15 и локальном узле на записи
//! (tests/lb_node): память — пул BTC.b/WBNB 0x717d…073d, прочитанный настоящим загрузчиком (load_lb::load) на блоке
//! probe.json (133716961); событие — настоящий лог Swap swap_log.json (блок 133727176, logIndex 0x160, бин 8399652 —
//! единственный лог пяти пулов в блоках 133726818–133732817, README записи; поэтому бины и активный бин на конце блока
//! лога = на блоке probe_time.json 133732452 — читаем там). Проверяется:
//! - пул с ОДНИМ логом в блоке уходит сразу: лог не открывает «в обновлении» (settle.rs step), план — один запрос
//!   (settle_read.rs calls: getBin бина события + getActiveId — 2 вызова, один try_many у узла), активный бин узла на
//!   конце блока = активному бину памяти → Settled::Done, отметки нет, достоверность прежняя, бин перечитан;
//! - п. 2 «найдено» ревью 332 (settle_head.rs, lb_head.rs): удачное чтение на голове НЕ поднимает достоверность пулу,
//!   недостоверному не по вине головы (ждущему повтора после StaleData, решение 298), и возвращает её пулу, у которого
//!   её сняла неудачная голова (head_failed → head_ok).
//!
//! Последовательности логов ОДНОГО обмена через несколько бинов (и SwapFee → Swap Integral, и нескольких Swap Infinity
//! Bin в блоке) в записях нет — тест «промежуточное состояние не уходит» ждёт записи (промпт — заметка
//! docs/notes/t154-settle-2026-11-16.md, раздел 5).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/native_common, tests/lb_node, arb::venues::bsc_dex::pools::{settle, settle_bins, settle_read,
//! settle_head, lb_head, events, load_lb, registry}, arb::venues::bsc_dex::node::call.
//! Настройки теста: узел (lb_node::attach), dex.bin_depth_usd = 3 [bsc_dex] (полоса загрузчика).

mod lb_node;
mod native_common;

use alloy::rpc::types::Log;
use alloy_primitives::{Address, B256, Bytes, LogData};
use serde_json::json;

use arb::core::types::{LbBinData, PoolRef, PoolState};
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::node::call::{block_tag, try_many};
use arb::venues::bsc_dex::pools::events::{self, Applied};
use arb::venues::bsc_dex::pools::registry::{Entry, Registry};
use arb::venues::bsc_dex::pools::settle::Holds;
use arb::venues::bsc_dex::pools::settle_bins::{self, Plan, Settled};
use arb::venues::bsc_dex::pools::{lb_head, load_lb, settle_read};
use arb::venues::bsc_dex::tokens::Tokens;
use lb_node::{Rec, addr};

const BTCB_WBNB: &str = "0x717d06b4d8bc6b71d5a2c45d8cc417930aec073d";

/// Узел на записи, окружение, записи probe / probe_time, токены с decimals записи.
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

/// Пул, прочитанный настоящим загрузчиком на блоке записи `r` (встречный токен — Y записи).
async fn loaded(c: &Ctx, t: &Tokens, r: &Rec, pool: &str) -> Entry {
    let a: Address = pool.parse().unwrap();
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

/// Настоящий лог Swap (swap_log.json) как лог подписки: адрес, topics, data, блок, logIndex — из записи.
fn swap_log() -> Log {
    let l = &lb_node::json_of("swap_log.json")["logs"][0];
    let topics: Vec<B256> = l["topics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t.as_str().unwrap().parse().unwrap())
        .collect();
    let raw = alloy_primitives::hex::decode(l["data"].as_str().unwrap()).unwrap();
    let index = u64::from_str_radix(l["logIndex"].as_str().unwrap().trim_start_matches("0x"), 16);
    Log {
        inner: alloy_primitives::Log {
            address: addr(&l["address"]),
            data: LogData::new(topics, Bytes::from(raw)).unwrap(),
        },
        block_number: Some(l["blockNumber"].as_u64().unwrap()),
        log_index: Some(index.unwrap()),
        ..Default::default()
    }
}

fn data(e: &Entry) -> &LbBinData {
    match &e.pool.state {
        PoolState::LbBin(d) => d,
        _ => panic!("не LB"),
    }
}

/// Пул с одним логом в блоке: «в обновлении» не открывается, один запрос, Done — ядру сразу.
#[tokio::test]
async fn single_log_goes_at_once_with_one_request() {
    let (c, probe, time, t) = scene("settle-357-one").await;
    let mem = loaded(&c, &t, &probe, BTCB_WBNB).await;
    let r = mem.pool.reference.clone();
    let family = mem.family;
    let (reg, holds) = (Registry::default(), Holds::default());
    reg.put(mem);
    let log = swap_log();
    let block = log.block_number.unwrap();
    let applied = reg.update(&r, |e| events::apply(e, &log)).unwrap();
    let Applied::Bins(ids) = &applied else {
        panic!("Swap LB — Bins, а не {applied:?}");
    };
    let ids = ids.clone();
    assert_eq!(ids, vec![8_399_652]);
    // Swap не открывает «в обновлении» (это не SwapFee / CompositionFees).
    assert!(!holds.step(&reg, (&r, family), &log, &applied, false));
    // План: бин события, без накопленных — один запрос из двух вызовов (getBin + getActiveId).
    let Plan::Read(list) = settle_bins::plan(&holds, &reg, &r, &ids, block) else {
        panic!("план — чтение");
    };
    assert_eq!(list, ids);
    assert_eq!(settle_read::calls(&r, &list).unwrap().len(), 2);
    // Чтение тем же try_many у узла на записи (конец блока лога = блок probe_time, README записи).
    let read = settle_read::read(&c, &r, &list, &block_tag(time.block))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read.1, 8_399_652, "активный бин конца блока = бину лога");
    let want = lb_node::bins_of(&time.pools[&r.address()])[&8_399_652];
    assert_eq!(read.0, vec![(8_399_652, want.0, want.1)]);
    assert_eq!(
        settle_bins::finish(&holds, &reg, &r, block, read),
        Settled::Done
    );
    let e = reg.get(&r).unwrap();
    assert!(e.valid, "достоверность прежняя");
    assert!(holds.get(&r).is_none(), "отметки нет — ядру сразу");
    assert_eq!(
        e.bins.as_ref().unwrap().reserves.get(&8_399_652),
        Some(&want)
    );
    assert_eq!(data(&e).active_id, 8_399_652);
    assert!(!holds.any());
}

/// Чтение на голове тем же путём, что per_block_lb.rs: lb_head::calls → try_many → lb_head::decode.
async fn head(c: &Ctx, e: &Entry, at: &Rec) -> (LbBinData, (u128, u128)) {
    let a = e.pool.reference.address();
    let calls = lb_head::calls(a, lb_head::active_of(e).unwrap());
    let got = try_many(c, &calls, &block_tag(at.block)).await.unwrap();
    let answers: Vec<Bytes> = got.into_iter().map(|r| r.unwrap()).collect();
    lb_head::decode(&answers, data(e), at.ts).unwrap()
}

/// П. 2 «найдено» ревью 332: голова не поднимает достоверность чужой недостоверности, но возвращает свою.
#[tokio::test]
async fn head_restores_only_its_own_validity() {
    let (c, probe, time, t) = scene("settle-357-head").await;
    // USDC/USDT: на голове всё то же, что в памяти (README записи) — «равны».
    let quiet = loaded(&c, &t, &probe, "0xaec97c3465d83b1952c3a251036df88a635c060c").await;
    let r = quiet.pool.reference.clone();
    let (d, bin) = head(&c, &quiet, &time).await;
    assert!(lb_head::same(&quiet, &d, bin));
    // Недостоверен не по вине головы (как после StaleData: listen_log.rs ставит valid = false) — голова не поднимает.
    let mut waiting = quiet.clone();
    waiting.valid = false;
    assert!(!lb_head::renewed(&waiting, time.block, d.clone()).valid);
    // Достоверный — остаётся достоверным.
    assert!(lb_head::renewed(&quiet, time.block, d.clone()).valid);
    // Отметка head: неудачная голова сняла достоверность — удачная возвращает её (lb_head::put по head_ok).
    let (reg, holds) = (Registry::default(), Holds::default());
    reg.put(quiet.clone());
    holds.head_failed(&reg, &r);
    assert!(!reg.get(&r).unwrap().valid);
    assert_eq!(holds.head_ok(&r), Some(true));
    assert!(holds.get(&r).is_none());
    // Уже недостоверный (ждёт повтора) и неудачная голова: отметки нет — удачная голова вернуть нечего.
    reg.update(&r, |e| e.valid = false);
    holds.head_failed(&reg, &r);
    assert_eq!(holds.head_ok(&r), None);
    assert!(!reg.get(&r).unwrap().valid);
}
