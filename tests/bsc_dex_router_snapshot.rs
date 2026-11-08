//! Файл: tests/bsc_dex_router_snapshot.rs
//! Что проверяет: исправления по проверке решения 217 — снимок DEX на момент решения в graph-режиме (dex.router_mode =
//! graph) на пулах РЕАЛЬНОЙ записи CAKE 2026-10-29 (tests/route_common: CAKE-USDT ×2, CAKE-WBNB, WBNB-USDT, CAKE-BTCB,
//! BTCB-WBNB; блок 130521896) плюс посторонний пул (копия записи WBNB-USDT под другой монетой — не на путях CAKE):
//! - снимок берёт только пулы, нужные расчёту по монете: без путей графа в кэше — минимальный набор (как прямой режим),
//!   с путями (dex.route_paths_ttl_ms) — ещё пулы этих путей; постороннего пула и полного реестра в нём нет;
//! - живой реестр после снимка не копирует запись постороннего пула при записи в него (копия — только у снятых);
//! - замороженный реестр берёт закреплённые пути момента снимка (без срока и без построения графа) и даёт ту же
//!   котировку графа, что живой до изменений рынка;
//! - пулы «WBNB — USDT» для цены газа попадают в снимок, даже если WBNB не среди промежуточных;
//! - порядок ошибок живой котировки DEX как до 217: dex.swap_mode проверяется раньше свежести блока;
//! - решение 236: цена газа снимка закреплена (quote/gas.rs pinned) — без цены в памяти ошибка, узел не спрашивается.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{frozen, pools::registry_frozen, quote::{graph_route, route_cache}},
//! arb::gateway::frozen, tests/route_common.

mod route_common;

use std::collections::HashSet;
use std::sync::Arc;

use alloy_primitives::{Address, U256, address};
use rust_decimal::Decimal;
use serde_json::json;

use arb::core::types::{Asset, PoolRef, Quote, Side, TokenTaxes};
use arb::gateway::errors::ErrorCode;
use arb::gateway::frozen::FrozenQuote;
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::frozen;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::pools::mids::Mid;
use arb::venues::bsc_dex::pools::registry::Registry;
use arb::venues::bsc_dex::pools::registry_frozen::Want;
use arb::venues::bsc_dex::quote::steps::Names;
use arb::venues::bsc_dex::quote::via::Via;
use arb::venues::bsc_dex::quote::{Quoting, graph_route, route_cache, route_paths};

use route_common::{BTCB, CAKE, CAKE_USDT_PCS, USDT, WBNB, WBNB_USDT_V2};

const GAS: u64 = 1_000_000_000;
const BLOCK: u64 = 130_521_896;
/// Посторонняя монета и её пул (копия записи WBNB-USDT): не на путях CAKE.
const OTHER: Address = address!("0x0700000000000000000000000000000000000005");
const OTHER_PAIR: Address = address!("0x0700000000000000000000000000000000000006");

async fn set(p: &Parts, name: &str, v: Option<serde_json::Value>) {
    p.ctx
        .settings
        .set(name, Scope::Venue("bsc_dex".into()), v, "тест")
        .await
        .unwrap();
}

fn cake() -> Asset {
    Asset {
        ticker: "CAKE".into(),
        bsc_contract: Some(CAKE),
    }
}

/// Окружение записи + посторонний пул, свежая голова, graph-режим. Ошибок нет.
async fn graph_parts() -> Parts {
    let p = route_common::parts().await;
    let mut other = p
        .registry
        .get(&PoolRef::Contract(WBNB_USDT_V2))
        .expect("пул WBNB-USDT записи");
    other.pool.reference = PoolRef::Contract(OTHER_PAIR);
    other.coin = OTHER;
    other.ticker = "OTHER".into();
    p.registry.put(other);
    set(&p, "feed.stale_after_ms", Some(json!(600_000))).await;
    set(&p, "dex.router_mode", Some(json!("graph"))).await;
    p.heads.observe(BLOCK);
    p
}

/// Котировка графа покупки на реестре окружения `q`. Ошибки — паника теста.
async fn graph_quote(q: &Quoting, qty: u32) -> Quote {
    let t = TokenTaxes {
        buy_pct: Decimal::ZERO,
        sell_pct: Decimal::ZERO,
        transfer_pct: Decimal::ZERO,
    };
    let v = Via {
        q,
        coin: CAKE,
        usdt: USDT,
        side: Side::Buy,
        taxes: &t,
        coin_dec: 18,
        usdt_dec: 18,
    };
    let names = Names {
        usdt: (USDT, "USDT"),
        coin: (CAKE, "CAKE"),
        mids: &q.mids,
    };
    graph_route::quote(&v, &names, &cake(), Decimal::from(qty), U256::from(GAS))
        .await
        .expect("котировка графа")
}

/// Адрес записи пула в живом реестре (без удержания `Arc`: снимок всех записей сразу отпускается). Ошибок нет.
fn ptr(reg: &Registry, r: &PoolRef) -> usize {
    reg.snapshot()
        .into_iter()
        .find(|e| e.pool.reference == *r)
        .map(|e| Arc::as_ptr(&e) as usize)
        .expect("пул в реестре")
}

#[tokio::test]
async fn graph_snapshot_only_coin_path_pools() {
    let p = graph_parts().await;
    assert_eq!(
        p.registry.snapshot().len(),
        7,
        "6 пулов записи + посторонний"
    );
    // Путей графа в кэше нет (dex.route_paths_ttl_ms не задана): минимальный набор — CAKE-USDT ×2, CAKE-WBNB,
    // WBNB-USDT (у CAKE-BTCB монета записи — BTCB, это пул пути, не пул монеты).
    let f = frozen::take(p.quoting(), &cake(), Side::Buy).expect("снимок");
    let d = f.describe();
    assert!(d.contains(": 4 записей"), "{d}");
    assert!(
        d.contains("путей графа в кэше на момент решения нет"),
        "{d}"
    );
    // Живой кэш путей монеты: снимок добавляет пулы путей (CAKE-BTCB, BTCB-WBNB), посторонний — нет.
    set(&p, "dex.route_paths_ttl_ms", Some(json!(60_000))).await;
    let live = graph_quote(&p.quoting(), 20_000).await;
    let f = frozen::take(p.quoting(), &cake(), Side::Buy).expect("снимок");
    let d = f.describe();
    assert!(d.contains(": 6 записей, путей графа"), "{d}");
    // Пока снимок жив: запись в посторонний пул не копирует его запись, в снятый — копирует (копия при записи).
    let other = PoolRef::Contract(OTHER_PAIR);
    let taken = PoolRef::Contract(CAKE_USDT_PCS);
    let (o1, t1) = (ptr(&p.registry, &other), ptr(&p.registry, &taken));
    p.registry.update(&other, |e| e.synced_block += 1);
    p.registry.update(&taken, |e| e.synced_block += 1);
    assert_eq!(ptr(&p.registry, &other), o1, "посторонний пул скопирован");
    assert_ne!(ptr(&p.registry, &taken), t1, "снятый пул не скопирован");
    drop(f);
    // Котировка графа после записи в пулы (состояние то же) — та же.
    assert_eq!(
        graph_quote(&p.quoting(), 20_000).await.avg_price,
        live.avg_price
    );
}

#[tokio::test]
async fn frozen_graph_uses_pinned_paths() {
    let p = graph_parts().await;
    set(&p, "dex.route_paths_ttl_ms", Some(json!(60_000))).await;
    let live = graph_quote(&p.quoting(), 20_000).await;
    let q = p.quoting();
    let mids: HashSet<Address> = [WBNB, BTCB].into_iter().collect();
    let (max_hops, max_paths) = route_paths::limits(&q.ctx).unwrap();
    let search = route_cache::Search {
        usdt: USDT,
        coin: CAKE,
        allowed: &mids,
        max_hops,
        max_paths,
        cap: None,
    };
    let (key, sk) = route_cache::lookup(&q.ctx, &q.reg, &search)
        .unwrap()
        .expect("пути монеты в живом кэше");
    assert!(sk.len() > 1, "у записи несколько путей");
    let mids_v: Vec<Address> = vec![WBNB, BTCB];
    let snap = q.reg.freeze(&Want {
        coin: CAKE,
        mids: &mids_v,
        usdt: USDT,
        wbnb: Some(WBNB),
        paths: &sk,
    });
    assert_eq!(snap.entries.len(), 6);
    // Замороженный реестр с путями момента снимка: та же котировка, что живая до изменений.
    let mut fq = p.quoting();
    fq.reg = Registry::frozen_from(&snap, Some((key.clone(), sk.clone())));
    fq.frozen_head = Some(BLOCK);
    assert_eq!(graph_quote(&fq, 20_000).await.avg_price, live.avg_price);
    // Закреплён только первый путь, dex.route_paths_ttl_ms снята: замороженный реестр берёт ровно его (граф по
    // реестру снимка не строится — он нашёл бы все пути).
    set(&p, "dex.route_paths_ttl_ms", None).await;
    let one = Arc::new(vec![sk[0].clone()]);
    let fz = Registry::frozen_from(&snap, Some((key, one)));
    let got = route_cache::paths(&q.ctx, &fz, &q.tokens, 18, BLOCK, &search).unwrap();
    assert_eq!(got.len(), 1);
    let all = route_cache::paths(&q.ctx, &q.reg, &q.tokens, 18, BLOCK, &search).unwrap();
    assert_eq!(all.len(), sk.len(), "живой без кэша — граф и перебор");
}

#[tokio::test]
async fn gas_pools_in_snapshot_without_wbnb_mid() {
    let p = graph_parts().await;
    set(&p, "dex.router_mode", Some(json!("classic"))).await;
    p.mids.set(vec![Mid {
        address: BTCB,
        ticker: "BTCB".into(),
    }]);
    // dex.native_wrapped не задан: пулов газа нет — CAKE-USDT ×2, CAKE-WBNB.
    let d = frozen::take(p.quoting(), &cake(), Side::Buy)
        .unwrap()
        .describe();
    assert!(d.contains(": 3 записей"), "{d}");
    // Задан: + WBNB-USDT (цена газа в USDT, как у живой котировки — quote/pick.rs, route_gas.rs).
    set(&p, "dex.native_wrapped", Some(json!(WBNB.to_string()))).await;
    let d = frozen::take(p.quoting(), &cake(), Side::Buy)
        .unwrap()
        .describe();
    assert!(d.contains(": 4 записей"), "{d}");
}

#[tokio::test]
async fn live_quote_checks_mode_before_freshness() {
    let p = route_common::parts().await;
    set(&p, "feed.stale_after_ms", Some(json!(600_000))).await;
    // Головы нет и dex.swap_mode не задана — ошибка про режим (как до 217), не про свежесть.
    let e = p
        .quoting()
        .quote(&cake(), Side::Buy, Decimal::ONE)
        .await
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotConfigured, "{}", e.message);
    assert_eq!(e.setting.as_deref(), Some("dex.swap_mode"));
    // Режим задан — дальше свежесть блока.
    set(&p, "dex.swap_mode", Some(json!("direct"))).await;
    let e = p
        .quoting()
        .quote(&cake(), Side::Buy, Decimal::ONE)
        .await
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::StaleData, "{}", e.message);
}

/// Вердикт гистерезиса глубины у замороженного реестра — на момент СНИМКА, а не из живой книги в момент расчёта.
#[tokio::test]
async fn frozen_depth_verdict_from_snapshot_moment() {
    let p = graph_parts().await;
    let q = p.quoting();
    let r = PoolRef::Contract(CAKE_USDT_PCS);
    let e = q.reg.get(&r).unwrap();
    let live = || q.reg.depth().depth(&q.ctx, &q.reg, &q.tokens, &e, 18);
    // Полоса пула записи — от неё пороги: порог ниже полосы — глубокий.
    let band = live().unwrap().band.buy;
    let min = |k: &str| {
        let v = (band * Decimal::from_str_exact(k).unwrap()).round_dp(2);
        Some(json!(v.to_string().parse::<f64>().unwrap()))
    };
    set(&p, "dex.depth_hysteresis_pct", Some(json!(50))).await;
    set(&p, "dex.min_pool_liquidity_usd", min("0.9")).await;
    assert!(live().unwrap().deep_buy, "глубокий на момент снимка");
    let snap = q.reg.freeze(&Want {
        coin: CAKE,
        mids: &[],
        usdt: USDT,
        wbnb: None,
        paths: &[],
    });
    // После снимка живая книга сменила вердикт: порог ×3 — мелкий даже с гистерезисом.
    set(&p, "dex.min_pool_liquidity_usd", min("3")).await;
    assert!(!live().unwrap().deep_buy, "живой — мелкий");
    // Порог ×1.5: с прошлым «глубоким» (момент снимка) — глубокий (гистерезис 50%), с живым «мелким» — нет.
    set(&p, "dex.min_pool_liquidity_usd", min("1.5")).await;
    let fz = Registry::frozen_from(&snap, None);
    let fd = fz.depth().depth(&q.ctx, &fz, &q.tokens, &e, 18).unwrap();
    assert!(fd.deep_buy, "вердикт снимка: глубокий");
    assert!(!live().unwrap().deep_buy, "живой остаётся мелким");
}

#[tokio::test]
async fn pinned_gas_never_goes_live() {
    // Решение 236: цена газа снимка закреплена на момент снимка; её не было в памяти — ошибка StaleData, а не запрос к
    // узлу и не живая цена позже.
    let p = graph_parts().await;
    let q = p.quoting();
    let e = q.gas.pinned().wei(&q.ctx, &q.heads).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::StaleData, "{}", e.message);
    assert!(e.message.contains("на момент снимка"), "{}", e.message);
}
