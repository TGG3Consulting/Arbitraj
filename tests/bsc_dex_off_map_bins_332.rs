//! Файл: tests/bsc_dex_off_map_bins_332.rs
//! Что делает: решение 332 (задача t154) — «цена ушла за прочитанную полосу бинов → пул перечитывается»
//! (pools/off_map_bins.rs check) на РЕАЛЬНОЙ записи Liquidity Book V2.1 tests/data/lb_v21_2026-11-15/probe.json
//! (блок 133716961): пул USDC/USDT 0xaec9…060c (binStep 1, активный 8388613, X = USDC, Y = USDT, decimals 18), все
//! 25 непустых бинов записаны (обход getNextNonEmptyBin до края в обе стороны). Полоса, которую прочитал бы загрузчик
//! при dex.bin_depth_usd = 3, строится тем же правилом, что load_lb_bins.rs band (от активного по записанным непустым
//! бинам, пока стоимость usdt_value стороны не достигнет 3; бины кончились — край 0 / 0xFFFFFF): выходит 8388611…8388617.
//! «После обмена» меняется только активный бин (на записанный id), резервы — записанные (как в
//! tests/bsc_dex_off_map_algebra.rs: проверяется правило полосы, не содержимое после обмена).
//! Проверяется: настройка не задана — false при любом активном (1 в 1 как раньше); при 50%: на прочитанном состоянии —
//! false (повтора сразу после чтения нет); активный +1 бин — вверх прочитано 1,587 ≥ 1,5 — false; +2 — 0,952 < 1,5 —
//! true; на краю и за краем полосы — true; −1 — вниз 3,255 — false; −2 (край) — true; полоса до конца пула — false;
//! настройка у другой DEX — false; dex.bin_depth_usd не задана — false; пара без USDT — false.
//! Infinity Bin (PoolState::Bin) — та же функция, но записи пула Infinity Bin нет: тест ждёт записи (docs/notes/
//! t154-off-map-all-kinds-2026-11-16.md, промпт серверному ИИ).
//! Кто вызывает: cargo test.
//! От чего зависит: tests/native_common (ctx, entry), arb::venues::bsc_dex::pools::{off_map_bins, load_lb, load_bin,
//! bins}, arb::settings.
//! Настройки теста: dex.bin_depth_usd = 3 [bsc_dex], dex.bin_reread_left_pct = 50 [DEX lb_v21] — по случаю.

mod native_common;

use std::collections::BTreeMap;

use alloy_primitives::{Address, Bytes, U256, address};
use rust_decimal::Decimal;
use serde_json::{Value, json};

use arb::core::types::PoolState;
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::pools::bins::BinMap;
use arb::venues::bsc_dex::pools::load_bin::{Depth, usdt_value};
use arb::venues::bsc_dex::pools::load_lb;
use arb::venues::bsc_dex::pools::off_map_bins::{SETTING, check};
use arb::venues::bsc_dex::pools::registry::Entry;

const PROBE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/data/lb_v21_2026-11-15/probe.json"
);
const POOL: &str = "0xaec97c3465d83b1952c3a251036df88a635c060c";
const USDT: Address = address!("0x55d398326f99059fF775485246999027B3197955");
const LIMIT: u32 = 3;

fn num(v: &Value) -> U256 {
    U256::from_str_radix(v.as_str().unwrap(), 10).unwrap()
}

/// Записанные слова ответа eth_call → байты ответа.
fn words(v: &[&Value]) -> Bytes {
    let mut b = Vec::new();
    for w in v {
        b.extend_from_slice(&num(w).to_be_bytes::<32>());
    }
    Bytes::from(b)
}

/// Запись пула и все его непустые бины (id → X, Y).
fn pool() -> (Value, Value, BTreeMap<u32, (u128, u128)>) {
    let probe: Value = serde_json::from_str(&std::fs::read_to_string(PROBE).unwrap()).unwrap();
    let p = probe["pools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["pool"] == POOL)
        .unwrap()
        .clone();
    let mut all = BTreeMap::new();
    for (k, v) in p["bins"].as_object().unwrap() {
        let x: u128 = v[0].as_str().unwrap().parse().unwrap();
        let y: u128 = v[1].as_str().unwrap().parse().unwrap();
        all.insert(k.parse::<u32>().unwrap(), (x, y));
    }
    (probe, p, all)
}

/// Полоса загрузчика (load_lb_bins.rs band) по записанным непустым бинам при пределе `limit`.
fn band(
    c: &Ctx,
    all: &BTreeMap<u32, (u128, u128)>,
    active: u32,
    step: u32,
    limit: Decimal,
) -> BinMap {
    let d = Depth {
        usdt_is_y: true,
        usdt_decimals: 18,
    };
    let mut range = (active, active);
    let mut sum = Decimal::ZERO;
    let mut end = true;
    for (id, (x, y)) in all.range(..active).rev() {
        range.0 = *id;
        sum += usdt_value(c, *x, *y, *id, step, &d).unwrap();
        if sum >= limit {
            end = false;
            break;
        }
    }
    if end {
        range.0 = 0;
    }
    let (mut sum, mut end) = (Decimal::ZERO, true);
    for (id, (x, y)) in all.range(active + 1..) {
        range.1 = *id;
        sum += usdt_value(c, *x, *y, *id, step, &d).unwrap();
        if sum >= limit {
            end = false;
            break;
        }
    }
    if end {
        range.1 = 0xff_ffff;
    }
    let read: BTreeMap<u32, (u128, u128)> = all
        .range(range.0..=range.1)
        .map(|(k, v)| (*k, *v))
        .collect();
    BinMap {
        step,
        reserves: read.into(),
        range,
    }
}

/// Запись реестра LB с записанным состоянием (load_lb::decode) и полосой `bins`.
fn entry(probe: &Value, p: &Value, bins: BinMap) -> Entry {
    let s: Vec<&Value> = p["static"].as_array().unwrap().iter().collect();
    let v: Vec<&Value> = p["variable"].as_array().unwrap().iter().collect();
    let answers = vec![words(&[&p["activeId"]]), words(&s), words(&v)];
    let x: Address = p["tokenX"].as_str().unwrap().parse().unwrap();
    let y: Address = p["tokenY"].as_str().unwrap().parse().unwrap();
    let step: u32 = p["binStep"].as_str().unwrap().parse().unwrap();
    let d = load_lb::decode(&answers, x < y, step, probe["timestamp"].as_u64().unwrap()).unwrap();
    let mut e = native_common::entry("lb_v21", None);
    e.pool.token0 = x.min(y);
    e.pool.token1 = x.max(y);
    e.pool.bin_step = Some(step);
    e.pool.state = PoolState::LbBin(Box::new(d));
    e.usdt = USDT;
    e.coin = x;
    e.bins = Some(bins);
    e
}

/// Та же запись с активным бином `id`.
fn moved(e: &Entry, id: u32) -> Entry {
    let mut out = e.clone();
    if let PoolState::LbBin(d) = &mut out.pool.state {
        d.active_id = id;
    }
    out
}

async fn set(c: &Ctx, name: &str, scope: Scope, v: Option<Value>) {
    c.settings.set(name, scope, v, "тест").await.unwrap();
}

#[tokio::test]
async fn bins_band_reread_rule() {
    let c = native_common::ctx("off-map-bins-332").await;
    let (probe, p, all) = pool();
    let active: u32 = p["activeId"].as_str().unwrap().parse().unwrap();
    assert_eq!(active, 8_388_613);
    let bins = band(&c, &all, active, 1, Decimal::from(LIMIT));
    assert_eq!(bins.range, (8_388_611, 8_388_617));
    let e = entry(&probe, &p, bins);
    let venue = Scope::Venue("bsc_dex".into());
    let dex = Scope::Dex("lb_v21".into());
    set(&c, "dex.bin_depth_usd", venue.clone(), Some(json!(LIMIT))).await;

    // Не задана — как раньше: ни при каком активном.
    for id in 8_388_609..=8_388_619 {
        assert!(
            !check(&c, &moved(&e, id), Some(18)),
            "не задана, активный {id}"
        );
    }
    set(&c, SETTING, dex.clone(), Some(json!(50))).await;
    let cases = [
        (8_388_613, false), // как прочитано: вниз 6,18, вверх 3,17 — повтора нет
        (8_388_614, false), // вверх 1,587 ≥ 1,5
        (8_388_615, true),  // вверх 0,952 < 1,5
        (8_388_617, true),  // край полосы: вверх 0
        (8_388_618, true),  // за краем
        (8_388_612, false), // вниз 3,255
        (8_388_611, true),  // край снизу: вниз 0
        (8_388_609, true),  // за краем снизу
    ];
    for (id, want) in cases {
        assert_eq!(check(&c, &moved(&e, id), Some(18)), want, "активный {id}");
    }
    // decimals встречного токена не известны — не считается.
    assert!(!check(&c, &moved(&e, 8_388_618), None));
    // Полоса до конца пула в обе стороны (все бины записи) — проверять нечего.
    let mut full = e.clone();
    full.bins = Some(BinMap {
        step: 1,
        reserves: all.clone().into(),
        range: (0, 0xff_ffff),
    });
    assert!(!check(&c, &moved(&full, 8_388_620), Some(18)));
    // Пара без USDT — загрузчик читает все бины: не действует.
    let mut other = moved(&e, 8_388_618);
    other.usdt = Address::ZERO;
    assert!(!check(&c, &other, Some(18)));
    // dex.bin_depth_usd не задана — не действует.
    set(&c, "dex.bin_depth_usd", venue.clone(), None).await;
    assert!(!check(&c, &moved(&e, 8_388_618), Some(18)));
    // Настройка у другой DEX — у этой не действует.
    set(&c, "dex.bin_depth_usd", venue, Some(json!(LIMIT))).await;
    set(&c, SETTING, dex, None).await;
    set(
        &c,
        SETTING,
        Scope::Dex("pancakeswap_infinity_bin".into()),
        Some(json!(50)),
    )
    .await;
    assert!(!check(&c, &moved(&e, 8_388_618), Some(18)));
}
