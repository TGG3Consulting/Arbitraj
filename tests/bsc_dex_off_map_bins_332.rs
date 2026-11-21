//! Файл: tests/bsc_dex_off_map_bins_332.rs
//! Что делает: решение 332 и его ревью (Н3) — «цена ушла за прочитанную полосу бинов → пул перечитывается»
//! (pools/off_map_bins.rs verdict / check) на РЕАЛЬНОЙ записи Liquidity Book V2.1 tests/data/lb_v21_2026-11-15 через
//! НАСТОЯЩИЙ загрузчик: load_lb::load (состояние + полоса load_lb_bins::band) идёт к локальному узлу, который отдаёт
//! только записанные ответы (tests/lb_node). Копии правила полосы в тесте нет.
//! Случаи (встречный токен записи Entry.usdt — «USDT» загрузчика и проверки):
//! 1. USDC/USDT 0xaec9…060c (X = USDC = token1 → x_is_token0 = false; USDT = Y; decimals 18): dex.bin_depth_usd = 3 →
//!    полоса 8388611…8388617 (вниз 6,18, вверх 3,17). Настоящее второе состояние — probe_time.json (блок 133732452):
//!    пул тот же, перечитанный загрузчиком на втором блоке — та же полоса, проверка — «не перечитывать».
//! 2. BTC.b/WBNB 0x717d…073d (X = BTC.b = token0 → x_is_token0 = true), встречный WBNB = Y (decimals 18): при
//!    dex.bin_depth_usd = 0,5 вниз прочитано до конца пула (0,047 WBNB всего), вверх — до 8399784 (0,51).
//!    Настоящий обмен между записями — лог Swap swap_log.json (блок 133727176, бин 8399652 — активный): затронутый бин
//!    перечитан у узла на блоке probe_time (тот же бин после обмена; других логов пяти пулов в 133726818–133732817 нет —
//!    README записи), проверка после события — «не перечитывать» (обмен внутри активного бина полосу не съел).
//! 3. Тот же пул, встречный BTC.b = X (USDT = X; decimals 8; так бывает, когда монета — WBNB, а BTC.b — промежуточная
//!    монета решения 52): dex.bin_depth_usd = 0,01 → вверх до конца пула (0,0044 BTC.b всего), вниз — до 8395911 (0,12).
//!    Во всех трёх — «перечитать», когда dex.bin_depth_usd подняли выше прочитанного (настоящий путь: каждое повышение
//!    даёт одну перечитку на пул при следующем событии), и «не перечитывать» при той же полосе.
//!
//! Плюс: настройка не задана / у другой DEX / dex.bin_depth_usd не задана — Keep; decimals встречного неизвестны —
//! Unknown (ревью 332, Н2 — не молча).
//! Числа стоимости посчитаны заранее по записи (цена бина (1 + binStep/10⁴)^(id − 2²³)); пороги взяты с запасом.
//! Кто вызывает: cargo test.
//! От чего зависит: tests/native_common (ctx, entry), tests/lb_node (узел на записи),
//! arb::venues::bsc_dex::pools::{off_map_bins, load_lb, load_lb_bins, events_lb, bins}, arb::venues::bsc_dex::tokens.
//! Настройки теста: узел (lb_node::attach), dex.bin_depth_usd [bsc_dex], dex.bin_reread_left_pct [DEX lb_v21].

mod lb_node;
mod native_common;

use alloy_primitives::{Address, B256, Bytes, LogData};
use serde_json::{Value, json};

use arb::core::types::{PoolRef, PoolState};
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::node::call::block_tag;
use arb::venues::bsc_dex::pools::bins::NONE_BELOW;
use arb::venues::bsc_dex::pools::off_map_bins::{SETTING, Verdict, check, verdict};
use arb::venues::bsc_dex::pools::registry::Entry;
use arb::venues::bsc_dex::pools::{events_lb, load_lb, load_lb_bins};
use arb::venues::bsc_dex::tokens::Tokens;
use lb_node::{Rec, addr};

const USDC_USDT: &str = "0xaec97c3465d83b1952c3a251036df88a635c060c";
const BTCB_WBNB: &str = "0x717d06b4d8bc6b71d5a2c45d8cc417930aec073d";

/// Окружение на настоящей SQLite и узел на записи (оба блока).
async fn scene(tag: &str) -> (Ctx, Rec, Rec) {
    let c = native_common::ctx(tag).await;
    let url = lb_node::node(vec![
        lb_node::rec("probe.json"),
        lb_node::rec("probe_time.json"),
    ])
    .await;
    lb_node::attach(&c, &url).await;
    (
        c,
        lb_node::rec("probe.json"),
        lb_node::rec("probe_time.json"),
    )
}

/// decimals токенов пятёрки пулов — записанные ответы decimals() (probe.json decimalsX / decimalsY).
fn tokens(probe: &Rec) -> Tokens {
    let t = Tokens::default();
    for p in probe.pools.values() {
        let d = |k: &str| p[k].as_str().unwrap().parse::<u8>().unwrap();
        t.remember_decimals(addr(&p["tokenX"]), d("decimalsX"));
        t.remember_decimals(addr(&p["tokenY"]), d("decimalsY"));
    }
    t
}

async fn set(c: &Ctx, name: &str, scope: Scope, v: Option<Value>) {
    c.settings.set(name, scope, v, "тест").await.unwrap();
}

async fn depth(c: &Ctx, v: Value) {
    set(
        c,
        "dex.bin_depth_usd",
        Scope::Venue("bsc_dex".into()),
        Some(v),
    )
    .await;
}

/// Запись реестра пула, прочитанного НАСТОЯЩИМ загрузчиком (load_lb::load) у узла на блоке записи `r`.
/// Принимает: окружение, токены, запись блока, пул, встречный токен записи.
async fn loaded(c: &Ctx, t: &Tokens, r: &Rec, pool: &str, counter: Address) -> Entry {
    let a: Address = pool.parse().unwrap();
    let p = &r.pools[&a];
    let (x, y) = (addr(&p["tokenX"]), addr(&p["tokenY"]));
    let pair = (x.min(y), x.max(y));
    let (state, bins) = load_lb::load(c, t, a, pair, counter, r.block)
        .await
        .unwrap();
    let mut e = native_common::entry("lb_v21", None);
    e.pool.reference = PoolRef::Contract(a);
    e.pool.token0 = pair.0;
    e.pool.token1 = pair.1;
    e.pool.bin_step = Some(p["binStep"].as_str().unwrap().parse().unwrap());
    e.pool.state = state;
    e.pool.block = r.block;
    e.bins = Some(bins);
    e.usdt = counter;
    e.coin = if counter == x { y } else { x };
    e.synced_block = r.block;
    e
}

fn x_is_token0(e: &Entry) -> bool {
    match &e.pool.state {
        PoolState::LbBin(d) => d.x_is_token0,
        _ => panic!("не LB"),
    }
}

/// Случай 1: USDC/USDT, USDT = Y, x_is_token0 = false; второе состояние — probe_time.json.
#[tokio::test]
async fn usdt_is_y_token1_second_state() {
    let (c, probe, time) = scene("off-map-bins-332-y").await;
    let t = tokens(&probe);
    let usdt = addr(&probe.pools[&USDC_USDT.parse::<Address>().unwrap()]["tokenY"]);
    depth(&c, json!(3)).await;
    let e = loaded(&c, &t, &probe, USDC_USDT, usdt).await;
    assert!(!x_is_token0(&e));
    assert_eq!(e.bins.as_ref().unwrap().range, (8_388_611, 8_388_617));
    // Не задана — как раньше.
    assert_eq!(verdict(&c, &e, Some(18)), Verdict::Keep);
    let dex = Scope::Dex("lb_v21".into());
    set(&c, SETTING, dex.clone(), Some(json!(50))).await;
    // Сразу после чтения — повтора нет (вниз 6,18, вверх 3,17 ≥ 1,5).
    assert!(!check(&c, &e, Some(18)));
    // Настоящее второе состояние: тот же пул загрузчиком на блоке probe_time — полоса та же, перечитывать не нужно.
    let later = loaded(&c, &t, &time, USDC_USDT, usdt).await;
    assert_eq!(later.bins, e.bins);
    assert!(!check(&c, &later, Some(18)));
    // dex.bin_depth_usd подняли до 10: порог 5 > вверх 3,17 — перечитать.
    depth(&c, json!(10)).await;
    assert_eq!(verdict(&c, &later, Some(18)), Verdict::Reread);
    // decimals встречного неизвестны — не молча (Н2).
    assert!(matches!(verdict(&c, &later, None), Verdict::Unknown(_)));
    // Пара без встречного токена записи — не действует.
    let mut other = later.clone();
    other.usdt = Address::ZERO;
    assert_eq!(verdict(&c, &other, Some(18)), Verdict::Keep);
    // dex.bin_depth_usd не задана — не действует.
    set(
        &c,
        "dex.bin_depth_usd",
        Scope::Venue("bsc_dex".into()),
        None,
    )
    .await;
    assert_eq!(verdict(&c, &later, Some(18)), Verdict::Keep);
    // Настройка у другой DEX — у этой не действует.
    depth(&c, json!(10)).await;
    set(&c, SETTING, dex, None).await;
    let inf = Scope::Dex("pancakeswap_infinity_bin".into());
    set(&c, SETTING, inf, Some(json!(50))).await;
    assert_eq!(verdict(&c, &later, Some(18)), Verdict::Keep);
}

/// Настоящий лог Swap (swap_log.json) → (затронутые бины, новый активный).
fn swap_log() -> (Vec<u32>, Option<u32>, u64) {
    let l = &lb_node::json_of("swap_log.json")["logs"][0];
    let topics: Vec<B256> = l["topics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t.as_str().unwrap().parse().unwrap())
        .collect();
    let raw = alloy_primitives::hex::decode(l["data"].as_str().unwrap()).unwrap();
    let data = LogData::new(topics.clone(), Bytes::from(raw)).unwrap();
    let (ids, active) = events_lb::bins_of(topics[0], &data).unwrap().unwrap();
    (ids, active, l["blockNumber"].as_u64().unwrap())
}

/// Случай 2: BTC.b/WBNB, x_is_token0 = true, встречный WBNB = Y; настоящий обмен между записями.
#[tokio::test]
async fn token0_x_real_swap_between_states() {
    let (c, probe, time) = scene("off-map-bins-332-swap").await;
    let t = tokens(&probe);
    let a: Address = BTCB_WBNB.parse().unwrap();
    let wbnb = addr(&probe.pools[&a]["tokenY"]);
    depth(&c, json!("0.5")).await;
    let mut e = loaded(&c, &t, &probe, BTCB_WBNB, wbnb).await;
    assert!(x_is_token0(&e));
    assert_eq!(e.bins.as_ref().unwrap().range, (0, 8_399_784));
    set(&c, SETTING, Scope::Dex("lb_v21".into()), Some(json!(50))).await;
    assert!(!check(&c, &e, Some(18)));
    // Событие: как listen_log.rs (ветка Bins) — активный бин из события, затронутые бины перечитаны у узла пачкой.
    let (ids, active, block) = swap_log();
    assert_eq!((ids.clone(), active), (vec![8_399_652], Some(8_399_652)));
    assert!(block > probe.block && block <= time.block);
    if let (PoolState::LbBin(d), Some(n)) = (&mut e.pool.state, active) {
        d.active_id = n;
    }
    for (id, x, y) in load_lb_bins::many(&c, a, &ids, &block_tag(time.block))
        .await
        .unwrap()
    {
        e.bins.as_mut().unwrap().set(id, x, y);
    }
    let pt = lb_node::bins_of(&time.pools[&a]);
    assert_eq!(
        e.bins.as_ref().unwrap().reserves.get(&8_399_652),
        pt.get(&8_399_652)
    );
    // Обмен внутри активного бина полосу не съел — перечитывать не нужно.
    assert!(!check(&c, &e, Some(18)));
    // dex.bin_depth_usd подняли до 2: порог 1 > вверх 0,51 — перечитать (вниз — до конца пула, не проверяется).
    depth(&c, json!(2)).await;
    assert_eq!(verdict(&c, &e, Some(18)), Verdict::Reread);
}

/// Случай 3: тот же пул, встречный BTC.b = X (usdt_is_y = false).
#[tokio::test]
async fn usdt_is_x() {
    let (c, probe, _) = scene("off-map-bins-332-x").await;
    let t = tokens(&probe);
    let a: Address = BTCB_WBNB.parse().unwrap();
    let btcb = addr(&probe.pools[&a]["tokenX"]);
    depth(&c, json!("0.01")).await;
    let e = loaded(&c, &t, &probe, BTCB_WBNB, btcb).await;
    assert_eq!(e.bins.as_ref().unwrap().range, (8_395_911, NONE_BELOW));
    set(&c, SETTING, Scope::Dex("lb_v21".into()), Some(json!(50))).await;
    assert!(!check(&c, &e, Some(8)));
    // dex.bin_depth_usd подняли до 1: порог 0,5 > вниз 0,12 — перечитать (вверх — до конца пула).
    depth(&c, json!(1)).await;
    assert_eq!(verdict(&c, &e, Some(8)), Verdict::Reread);
}
