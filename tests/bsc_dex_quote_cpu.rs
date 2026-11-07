//! Файл: tests/bsc_dex_quote_cpu.rs
//! Что делает: решение 131 — котировка DEX на большом реестре: (1) пары и тикеры ищутся по индексу реестра, копия записи
//! делит карту тиков с реестром (нет копии тиков), итог котировки с большим реестром тот же, что с малым, и укладывается
//! во время; (2) выбор маршрута идёт вне рабочих потоков tokio (quote/cpu.rs): во время тяжёлого счёта другая задача
//! рантайма получает управление, а срок (tokio::time::timeout, как profit.calc_timeout_ms ядра) срабатывает вовремя.
//! Данные — запись маршрута CAKE (tests/data/routes_2026-10-29, route_common): шесть настоящих пулов. «Большой реестр» —
//! те же записанные состояния пула CAKE-USDT (PancakeSwap V3) под другими условными адресами монет (как у MEXC — тысячи
//! монет со своими пулами USDT): к маршруту CAKE они не ведут, но лежат в реестре и в графе.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex (quote::{cpu, routing}, pools::registry), tests/route_common.

mod route_common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;
use serde_json::json;

use arb::core::types::{Asset, PoolRef, Quote, Side, TokenTaxes};
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::quote::{Quoting, cpu, routing};

use route_common::{CAKE, CAKE_USDT_PCS, USDT};

const GAS: u64 = 1_000_000_000;

/// Окружение записи маршрута в режиме графа (dex.router_mode = graph, dex.swap_mode = direct).
async fn parts() -> Parts {
    let p = route_common::parts().await;
    let v = Scope::Venue("bsc_dex".into());
    for (n, val) in [
        ("dex.router_mode", json!("graph")),
        ("dex.swap_mode", json!("direct")),
    ] {
        p.ctx
            .settings
            .set(n, v.clone(), Some(val), "тест")
            .await
            .unwrap();
    }
    p
}

/// Входные данные выбора маршрута для покупки CAKE (как их собирает quote/mod.rs).
fn input(q: &Quoting, qty: u32) -> routing::Input {
    let entries = q
        .reg
        .by_pair(CAKE, USDT)
        .into_iter()
        .filter(|e| e.ticker == "CAKE")
        .collect();
    let mids = q.mids.active(&q.ctx).into_iter().map(|m| (m, 18)).collect();
    routing::Input {
        asset: Asset {
            ticker: "CAKE".into(),
            bsc_contract: Some(CAKE),
        },
        side: Side::Buy,
        qty: Decimal::from(qty),
        coin: CAKE,
        usdt: USDT,
        taxes: TokenTaxes {
            buy_pct: Decimal::ZERO,
            sell_pct: Decimal::ZERO,
            transfer_pct: Decimal::ZERO,
        },
        coin_dec: 18,
        usdt_dec: 18,
        entries,
        mids,
        gas: U256::from(GAS),
        usdt_ticker: "USDT".into(),
        smart: false,
    }
}

/// Выбор маршрута через cpu::run (как Quoting::quote).
async fn quote(q: &Quoting, qty: u32) -> Quote {
    cpu::run(&q.ctx, routing::run(q.clone(), input(q, qty)))
        .await
        .expect("котировка")
}

/// Добавляет в реестр `n` пулов «монета — USDT» с записанным состоянием пула CAKE-USDT PancakeSwap V3 под условными
/// адресами монет (меньше адреса USDT — та же ориентация token0/token1, что у записанного sqrtPriceX96).
fn bulk(p: &Parts, from: u32, n: u32) {
    let src = p.registry.get(&PoolRef::Contract(CAKE_USDT_PCS)).unwrap();
    for i in from..from + n {
        let mut coin = [0u8; 20];
        coin[0] = 0x10;
        coin[16..20].copy_from_slice(&i.to_be_bytes());
        let coin = Address::from(coin);
        let mut pool = [0u8; 20];
        pool[0] = 0xa0;
        pool[16..20].copy_from_slice(&i.to_be_bytes());
        let mut e = src.clone();
        e.coin = coin;
        e.ticker = format!("BULK{i}");
        e.pool.reference = PoolRef::Contract(Address::from(pool));
        e.pool.token0 = coin;
        e.pool.token1 = USDT;
        p.registry.put(e);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn big_registry_same_quote_no_copy_in_time() {
    let p = parts().await;
    let q = p.quoting();
    let small = quote(&q, 5_000).await;
    let ts = Instant::now();
    let _ = quote(&q, 5_000).await;
    let small_warm = ts.elapsed();
    bulk(&p, 0, 3_000);
    // Индекс пары: только пулы CAKE — USDT, а копия записи делит карту тиков с реестром (Arc, без копии тиков).
    let pair = q.reg.by_pair(CAKE, USDT);
    assert_eq!(pair.len(), 2);
    let again = q.reg.by_pair(CAKE, USDT);
    for (a, b) in pair.iter().zip(&again) {
        let (ta, tb) = (a.ticks.as_ref().unwrap(), b.ticks.as_ref().unwrap());
        assert!(
            Arc::ptr_eq(&ta.ticks, &tb.ticks),
            "копия записи скопировала тики"
        );
    }
    // Решение 211: пулы монеты — по адресу; запись с тем же тикером «CAKE», но другим адресом — другая монета.
    let mut bulk7 = [0u8; 20];
    bulk7[0] = 0x10;
    bulk7[16..20].copy_from_slice(&7u32.to_be_bytes());
    assert_eq!(q.reg.by_coin(&Address::from(bulk7)).len(), 1);
    // Пулы CAKE — 3 (route_common: у записей промежуточных монет тоже тикер «CAKE» — по тикеру было 6, по адресу 3).
    assert_eq!(q.reg.by_coin(&CAKE).len(), 3);
    let mut twin = p.registry.get(&PoolRef::Contract(CAKE_USDT_PCS)).unwrap();
    twin.coin = Address::from(bulk7);
    twin.ticker = "CAKE".into();
    twin.pool.reference = PoolRef::Contract(Address::repeat_byte(0xa9));
    twin.pool.token0 = twin.coin;
    p.registry.put(twin);
    assert_eq!(q.reg.by_coin(&CAKE).len(), 3, "тикер-двойник попал к CAKE");
    assert_eq!(q.reg.by_coin(&Address::from(bulk7)).len(), 2);
    // Первая котировка после записи строит граф (один раз на состояние реестра), следующие берут готовый.
    let t0 = Instant::now();
    let big = quote(&q, 5_000).await;
    let cold = t0.elapsed();
    let t1 = Instant::now();
    let warm_big = quote(&q, 5_000).await;
    let warm = t1.elapsed();
    // Одна новая запись (как событие пула): граф перестраивается, глубина прочих пулов — из памяти (решение 73).
    bulk(&p, 3_000, 1);
    let t2 = Instant::now();
    let _ = quote(&q, 5_000).await;
    let rebuilt = t2.elapsed();
    eprintln!("малый реестр: котировка {small_warm:?}");
    eprintln!(
        "большой реестр: первая котировка {cold:?}, следующая {warm:?}, после записи {rebuilt:?}"
    );
    for x in [&big, &warm_big] {
        assert_eq!(x.fillable_qty, small.fillable_qty);
        assert_eq!(x.avg_price, small.avg_price);
        assert_eq!(x.route, small.route);
    }
    // Граф уже построен: котировка на тысячах пулов реестра не проходит реестр — далеко в пределах срока расчёта.
    assert!(warm < Duration::from_secs(1), "{warm:?}");
}

#[tokio::test]
async fn heavy_quote_does_not_block_runtime_and_timeout_is_honest() {
    // Рантайм одного потока (как худший случай): без выноса счёта задача-счётчик не получила бы управления до его конца.
    let p = parts().await;
    let q = p.quoting();
    let _ = quote(&q, 5_000).await;
    bulk(&p, 0, 4_000);
    let ticks = Arc::new(AtomicU64::new(0));
    let t = ticks.clone();
    let ticker = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(1)).await;
            t.fetch_add(1, Ordering::Relaxed);
        }
    });
    let t0 = Instant::now();
    let _ = quote(&q, 5_000).await;
    let heavy = t0.elapsed();
    let seen = ticks.load(Ordering::Relaxed);
    eprintln!("тяжёлая котировка {heavy:?}, тиков другой задачи за это время: {seen}");
    assert!(
        heavy > Duration::from_millis(20),
        "счёт слишком лёгкий для проверки: {heavy:?}"
    );
    assert!(
        seen >= 5,
        "задача рантайма не получала управления во время счёта"
    );
    // Срок: новая запись реестра — граф снова холодный; срок короче счёта срабатывает вовремя, а не после счёта.
    bulk(&p, 4_000, 1);
    let t1 = Instant::now();
    let r = tokio::time::timeout(Duration::from_millis(5), quote(&q, 5_000)).await;
    let waited = t1.elapsed();
    eprintln!("срок 5 мс сработал через {waited:?}");
    assert!(
        r.is_err(),
        "счёт уложился в 5 мс — проверка срока не показательна"
    );
    assert!(
        waited < heavy / 2,
        "срок сработал только после счёта: {waited:?} при счёте {heavy:?}"
    );
    ticker.abort();
}
