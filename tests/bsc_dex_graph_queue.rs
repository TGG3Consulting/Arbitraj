//! Файл: tests/bsc_dex_graph_queue.rs
//! Что делает: решение 301 (задача t132, живой бот 15.11.2026: расчёт висел 8,5–10 с без запросов к узлу) — на
//! РЕАЛЬНОЙ записи маршрута CAKE (tests/route_common, routes_2026-10-29, блок 130521896):
//! (а) очередь построений графа: запрос, прочитавший версию реестра раньше, чем граф был построен на более новой
//!     версии (так видит кэш дождавшийся замка построения, пока версия росла), берёт готовый граф и НЕ строит его
//!     заново (счётчик построений Cache::builds); запрос с более новой версией или другими настройками графа — строит,
//!     как раньше (решение 301 (е): блок головы в ключ графа не входит);
//! (б) неудачная глубина пула (расчёт невозможен) считается один раз на ключ глубины: повторные построения графа её не
//!     пересчитывают (счётчик Book::calcs), причина та же; сменилось состояние пула (ключ) — пересчёт;
//! (в) брошенный расчёт (признак quote/abandon.rs поднят — срок расчёта вышел) не ждёт замок и не строит граф
//!     (Timeout, счётчик построений не растёт); живой расчёт в той же обёртке строит как обычно;
//! (д) счётчики замера построения (graph::build_measured): N — пулов в реестре, R — глубин пересчитано (второе
//!     построение на том же реестре — 0), граф тот же, что у build.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::pools::{graph, registry}, tests/route_common.

mod route_common;

use std::sync::Arc;

use serde_json::json;

use arb::core::types::PoolRef;
use arb::gateway::errors::ErrorCode;
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::pools::graph;
use arb::venues::bsc_dex::quote::abandon;

use route_common::{CAKE_USDT_PCS, WBNB_USDT_V2};

#[tokio::test]
async fn waiter_takes_newer_graph_without_rebuild() {
    let p = route_common::parts().await;
    let reg = &p.registry;
    let cache = reg.graph_cache();
    // Запрос «прочитал» версию до записи в реестр (ждал замок построения).
    let asked = reg.version();
    // Запись в реестр (то же состояние пула) — версия выросла, как от события пула во время ожидания.
    let r = PoolRef::Contract(WBNB_USDT_V2);
    reg.update(&r, |e| e.synced_log = Some(1))
        .expect("пул записи");
    assert!(reg.version() > asked);
    let built = graph::cached(&p.ctx, reg, &p.tokens, 18).unwrap();
    assert_eq!(cache.builds(), 1);
    assert!(!built.is_empty(), "граф записи не пуст");
    // Дождавшийся с более старой версией — тот же граф, без построения.
    let g = graph::cached_as_of(&p.ctx, reg, &p.tokens, 18, asked).unwrap();
    assert!(Arc::ptr_eq(&g, &built), "граф не старше запроса — из кэша");
    // Решение 301 (е): блока в ключе нет — повторный запрос на той же версии тоже из кэша.
    let same = graph::cached(&p.ctx, reg, &p.tokens, 18).unwrap();
    assert!(Arc::ptr_eq(&same, &built));
    assert_eq!(cache.builds(), 1, "повторного построения нет");
    // Граф новее запроса равен построенному заново на текущем реестре (те же рёбра).
    let fresh = graph::build(&p.ctx, reg, &p.tokens, 18).unwrap();
    assert_eq!(built.len(), fresh.len());
    // Версия запроса выше, чем у графа, — строится заново.
    reg.update(&r, |e| e.synced_log = Some(2)).unwrap();
    let newer = graph::cached(&p.ctx, reg, &p.tokens, 18).unwrap();
    assert!(!Arc::ptr_eq(&newer, &built));
    assert_eq!(cache.builds(), 2);
    // Сменилась настройка графа — строится заново даже при старой версии запроса.
    p.ctx
        .settings
        .set(
            "dex.route_max_pools_per_edge",
            Scope::Venue("bsc_dex".into()),
            Some(json!(4)),
            "тест",
        )
        .await
        .unwrap();
    graph::cached_as_of(&p.ctx, reg, &p.tokens, 18, asked).unwrap();
    assert_eq!(
        cache.builds(),
        3,
        "другие настройки графа — новое построение"
    );
}

#[tokio::test]
async fn failed_depth_counted_once_per_key() {
    let p = route_common::parts().await;
    let reg = &p.registry;
    let book = reg.depth();
    // Пул Pancake V3 записи без карты тиков: глубина не считается («карта тиков не прочитана»).
    let r = PoolRef::Contract(CAKE_USDT_PCS);
    let mut e = reg.get(&r).expect("пул записи");
    let ticks = e.ticks.take();
    assert!(ticks.is_some(), "в записи карта тиков есть");
    // Новое состояние пула (следующий блок) — ключ глубины другой, прежняя удачная глубина из книги не годится.
    e.synced_block += 1;
    e.synced_log = None;
    reg.put(e.clone());
    // Решение 301 (е2): поколение присваивает реестр — запись берём из него.
    let mut e = reg.get(&r).unwrap();
    let before_fail = book.calcs();
    let why = book.depth(&p.ctx, reg, &p.tokens, &e, 18).unwrap_err();
    assert!(why.contains("карта тиков"), "{why}");
    assert_eq!(book.calcs(), before_fail + 1, "неудача посчитана один раз");
    let after_first = book.calcs();
    // Повторный запрос глубины и повторные построения графа на том же ключе — без пересчёта, причина та же.
    assert_eq!(book.depth(&p.ctx, reg, &p.tokens, &e, 18).unwrap_err(), why);
    let g1 = graph::build(&p.ctx, reg, &p.tokens, 18).unwrap();
    let mid = book.calcs();
    let g2 = graph::build(&p.ctx, reg, &p.tokens, 18).unwrap();
    assert_eq!(
        book.calcs(),
        mid,
        "второе построение не пересчитывает ни одной глубины"
    );
    assert_eq!(g1.len(), g2.len());
    assert!(
        g2.between(e.coin, e.usdt)
            .iter()
            .all(|x| x.entry.pool.reference != r),
        "пул без глубины в граф не входит"
    );
    // Первый build посчитал только пулы, которых ещё не было в книге (их глубину на этом ключе); неудачный — нет.
    assert!(mid >= after_first);
    assert_eq!(book.depth(&p.ctx, reg, &p.tokens, &e, 18).unwrap_err(), why);
    assert_eq!(book.calcs(), mid, "неудача на том же ключе не пересчитана");
    // Состояние пула сменилось (карта тиков прочитана заново на следующем блоке) — ключ другой, глубина пересчитана.
    e.ticks = ticks;
    e.synced_block += 1;
    e.synced_log = None;
    reg.put(e.clone());
    let e = reg.get(&r).unwrap();
    let before = book.calcs();
    assert!(book.depth(&p.ctx, reg, &p.tokens, &e, 18).is_ok());
    assert_eq!(book.calcs(), before + 1);
}

#[tokio::test]
async fn abandoned_calc_does_not_build() {
    let p = route_common::parts().await;
    let reg = &p.registry;
    let cache = reg.graph_cache();
    // Срок вышел (страж брошен) — граф не строится, ошибка Timeout.
    let (g, flag) = abandon::guard();
    drop(g);
    let got = abandon::scope(flag, async { graph::cached(&p.ctx, reg, &p.tokens, 18) }).await;
    assert_eq!(got.err().map(|e| e.code), Some(ErrorCode::Timeout));
    assert_eq!(cache.builds(), 0, "брошенный расчёт граф не строит");
    // Живой расчёт (страж держится) — граф строится; итог равен построению без обёртки.
    let (_g, flag) = abandon::guard();
    let live = abandon::scope(flag, async { graph::cached(&p.ctx, reg, &p.tokens, 18) })
        .await
        .unwrap();
    assert_eq!(cache.builds(), 1);
    assert_eq!(
        live.len(),
        graph::build(&p.ctx, reg, &p.tokens, 18).unwrap().len()
    );
}

#[tokio::test]
async fn build_measured_counts_pools_and_recomputed() {
    let p = route_common::parts().await;
    let reg = &p.registry;
    let (g1, s1) = graph::build_measured(&p.ctx, reg, &p.tokens, 18).unwrap();
    assert_eq!(s1.pools, reg.all().len());
    let (g2, s2) = graph::build_measured(&p.ctx, reg, &p.tokens, 18).unwrap();
    assert_eq!(s2.pools, s1.pools);
    assert_eq!(
        s2.recomputed, 0,
        "реестр не менялся — все глубины из памяти"
    );
    assert_eq!(g1.len(), g2.len());
    assert_eq!(
        g2.len(),
        graph::build(&p.ctx, reg, &p.tokens, 18).unwrap().len()
    );
}
