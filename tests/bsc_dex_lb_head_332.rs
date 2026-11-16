//! Файл: tests/bsc_dex_lb_head_332.rs
//! Что делает: ревью решения 332 (Б1, деньги) — чтение Liquidity Book V2.1 «каждым блоком» (pools/lb_head.rs, его зовёт
//! per_block_lb.rs) на паре РЕАЛЬНЫХ записанных состояний tests/data/lb_v21_2026-11-15: память — пул, прочитанный
//! настоящим загрузчиком (load_lb::load) на блоке probe.json (133716961); голова — блок probe_time.json (133732452).
//! Между ними у пула BTC.b/WBNB 0x717d…073d был настоящий обмен (swap_log.json, блок 133727176, logIndex 0x160): активный
//! бин тот же (8399652), его резервы и параметры комиссии изменились; у USDC/USDT 0xaec9…060c не было ничего.
//! Вызовы головы (lb_head::calls: состояние + getBin(активный бин памяти)) идут к локальному узлу на записи (tests/lb_node)
//! тем же node/call.rs try_many, разбор — lb_head::decode. Проверяется:
//! - USDC/USDT: чтение согласовано с памятью (same) — запись lb_head::renewed: комиссия и время блока головы, активный
//!   бин и бины — памяти, отметка учтённого (synced_block / synced_log) прежняя;
//! - BTC.b/WBNB, голова раньше лога (порядок квитанций): НЕ согласовано (резервы активного бина у узла другие) — память
//!   не трогается; прежний путь (load::renewed на голове, как было до ревью) отметил бы блок головы учтённым — настоящий
//!   лог обмена был бы отброшен (has_applied = true) и устаревший резерв активного бина остался бы навсегда (Б1);
//!   при правиле lb_head лог не отброшен (has_applied = false);
//! - тот же пул после применения лога (как listen_log.rs ветка Bins: бин события перечитан у узла): согласовано — комиссия
//!   головы записывается (порядок «лог раньше головы» сходится к тому же состоянию).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/native_common, tests/lb_node, arb::venues::bsc_dex::pools::{lb_head, load, load_lb,
//! load_lb_bins, registry}, arb::venues::bsc_dex::node::call.
//! Настройки теста: узел (lb_node::attach), dex.bin_depth_usd = 3 [bsc_dex] (полоса — как у загрузчика; на правило
//! головы не влияет).

mod lb_node;
mod native_common;

use alloy_primitives::{Address, Bytes};
use serde_json::json;

use arb::core::types::{LbBinData, PoolRef, PoolState};
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::node::call::{block_tag, try_many};
use arb::venues::bsc_dex::pools::load::{self, Loaded};
use arb::venues::bsc_dex::pools::registry::Entry;
use arb::venues::bsc_dex::pools::{lb_head, load_lb, load_lb_bins};
use arb::venues::bsc_dex::tokens::Tokens;
use lb_node::{Rec, addr};

const USDC_USDT: &str = "0xaec97c3465d83b1952c3a251036df88a635c060c";
const BTCB_WBNB: &str = "0x717d06b4d8bc6b71d5a2c45d8cc417930aec073d";
/// Настоящий лог обмена BTC.b/WBNB (swap_log.json): блок и logIndex.
const SWAP_BLOCK: u64 = 133_727_176;
const SWAP_INDEX: u64 = 0x160;

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

fn data(e: &Entry) -> &LbBinData {
    match &e.pool.state {
        PoolState::LbBin(d) => d,
        _ => panic!("не LB"),
    }
}

/// Чтение головы тем же путём, что per_block_lb.rs: lb_head::calls → try_many у узла → lb_head::decode (время блока —
/// записанное время блока головы, как eth_getBlockByNumber).
async fn head(c: &Ctx, e: &Entry, at: &Rec) -> (LbBinData, (u128, u128)) {
    let a = e.pool.reference.address();
    let calls = lb_head::calls(a, lb_head::active_of(e).unwrap());
    let got = try_many(c, &calls, &block_tag(at.block)).await.unwrap();
    let answers: Vec<Bytes> = got.into_iter().map(|r| r.unwrap()).collect();
    lb_head::decode(&answers, data(e), at.ts).unwrap()
}

#[tokio::test]
async fn head_never_moves_bins_nor_swallows_block_logs() {
    let c = native_common::ctx("lb-head-332").await;
    let (probe, time) = (lb_node::rec("probe.json"), lb_node::rec("probe_time.json"));
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
    let t = Tokens::default();
    for p in probe.pools.values() {
        let d = |k: &str| p[k].as_str().unwrap().parse::<u8>().unwrap();
        t.remember_decimals(addr(&p["tokenX"]), d("decimalsX"));
        t.remember_decimals(addr(&p["tokenY"]), d("decimalsY"));
    }

    // USDC/USDT: на голове всё то же — согласовано, пишется комиссия и время головы, отметка прежняя.
    let quiet = loaded(&c, &t, &probe, USDC_USDT).await;
    let (d, bin) = head(&c, &quiet, &time).await;
    assert!(lb_head::same(&quiet, &d, bin));
    let w = lb_head::renewed(&quiet, time.block, d.clone());
    assert_eq!(data(&w).block_ts, time.ts);
    assert_eq!(data(&w).active_id, data(&quiet).active_id);
    assert_eq!(w.bins, quiet.bins);
    assert_eq!((w.synced_block, w.synced_log), (probe.block, None));
    assert_eq!(w.pool.block, time.block);

    // BTC.b/WBNB: голова раньше лога обмена — не согласовано (резерв активного бина у узла другой).
    let mut busy = loaded(&c, &t, &probe, BTCB_WBNB).await;
    let (d, bin) = head(&c, &busy, &time).await;
    assert_eq!(
        d.active_id,
        data(&busy).active_id,
        "активный бин тот же — обмен внутри него"
    );
    assert!(
        !lb_head::same(&busy, &d, bin),
        "резервы активного бина изменились"
    );
    assert_ne!(d.time_of_last_update, data(&busy).time_of_last_update);
    // Как было до ревью (load::renewed на голове): блок головы учтён — настоящий лог обмена отброшен бы (Б1).
    let old = load::renewed(
        &busy,
        time.block,
        Loaded {
            state: PoolState::LbBin(Box::new(d.clone())),
            ticks: None,
            bins: busy.bins.clone(),
            fee_pips: None,
        },
    );
    assert!(
        old.has_applied(SWAP_BLOCK, SWAP_INDEX),
        "прежний путь глотал лог блока"
    );
    // Правило lb_head: память не тронута — лог применится.
    assert!(!busy.has_applied(SWAP_BLOCK, SWAP_INDEX));

    // Лог применён (listen_log.rs, ветка Bins): бин события перечитан у узла (запись после обмена — probe_time; других
    // логов пула между блоками нет, README записи). Теперь голова согласована — комиссия головы записывается.
    let a: Address = BTCB_WBNB.parse().unwrap();
    for (id, x, y) in load_lb_bins::many(&c, a, &[8_399_652], &block_tag(time.block))
        .await
        .unwrap()
    {
        busy.bins.as_mut().unwrap().set(id, x, y);
    }
    busy.synced_block = SWAP_BLOCK;
    busy.synced_log = Some(SWAP_INDEX);
    let (d, bin) = head(&c, &busy, &time).await;
    assert!(lb_head::same(&busy, &d, bin));
    let w = lb_head::renewed(&busy, time.block, d.clone());
    assert_eq!(data(&w).time_of_last_update, d.time_of_last_update);
    assert_eq!(
        (w.synced_block, w.synced_log),
        (SWAP_BLOCK, Some(SWAP_INDEX))
    );
    assert_eq!(w.bins, busy.bins);
}
