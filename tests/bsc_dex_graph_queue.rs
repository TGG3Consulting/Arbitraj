//! Файл: tests/bsc_dex_graph_queue.rs
//! Что делает: решение 301 (задача t132, живой бот 15.11.2026: расчёт висел 8,5–10 с без запросов к узлу) — на
//! РЕАЛЬНОЙ записи маршрута CAKE (tests/route_common, routes_2026-10-29, блок 130521896):
//! (а) очередь построений графа: запрос, прочитавший версию реестра раньше, чем граф был построен на более новой
//!     версии (так видит кэш дождавшийся замка построения, пока версия росла), берёт готовый граф и НЕ строит его
//!     заново (счётчик построений Cache::builds); запрос с более новой версией, более высоким блоком головы или другими
//!     настройками графа — строит, как раньше;
//! (б) неудачная глубина пула (расчёт невозможен) считается один раз на ключ глубины: повторные построения графа её не
//!     пересчитывают (счётчик Book::calcs), причина та же; сменилось состояние пула (ключ) — пересчёт;
//! (в) брошенный расчёт (признак quote/abandon.rs поднят — срок расчёта вышел) не ждёт замок и не строит граф
//!     (Timeout, счётчик построений не растёт); живой расчёт в той же обёртке строит как обычно.
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

/// Блок записи routes_2026-10-29.
const HEAD: u64 = 130_521_896;

#[tokio::test]
async fn waiter_takes_newer_graph_without_rebuild() {
    let p = route_common::parts().await;
    let reg = &p.registry;
    let cache = reg.graph_cache();
    // Запрос «прочитал» версию до записи в реестр (ждал замок построения).
    let asked = reg.version();
    // Запись в реестр (то же состояние пула) — версия выросла, как от события пула во время ожидания.
    let r = PoolRef::Contract(WBNB_USDT_V2);
    reg.update(&r, |_| ()).expect("пул записи");
    assert!(reg.version() > asked);
    let built = graph::cached(&p.ctx, reg, &p.tokens, 18, HEAD).unwrap();
    assert_eq!(cache.builds(), 1);
    assert!(!built.is_empty(), "граф записи не пуст");
    // Дождавшийся с более старой версией (и тем же или более старым блоком) — тот же граф, без построения.
    for head in [HEAD, HEAD - 1, 0] {
        let g = graph::cached_as_of(&p.ctx, reg, &p.tokens, 18, head, asked).unwrap();
        assert!(Arc::ptr_eq(&g, &built), "граф не старше запроса — из кэша");
    }
    assert_eq!(cache.builds(), 1, "повторного построения нет");
    // Граф новее запроса равен построенному заново на текущем реестре (те же рёбра).
    let fresh = graph::build(&p.ctx, reg, &p.tokens, 18).unwrap();
    assert_eq!(built.len(), fresh.len());
    // Голова запроса выше, чем у графа, — строится заново (как раньше).
    let up = graph::cached(&p.ctx, reg, &p.tokens, 18, HEAD + 1).unwrap();
    assert!(!Arc::ptr_eq(&up, &built));
    assert_eq!(cache.builds(), 2);
    // Версия запроса выше, чем у графа, — строится заново.
    reg.update(&r, |_| ()).unwrap();
    let newer = graph::cached(&p.ctx, reg, &p.tokens, 18, HEAD + 1).unwrap();
    assert!(!Arc::ptr_eq(&newer, &up));
    assert_eq!(cache.builds(), 3);
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
    graph::cached_as_of(&p.ctx, reg, &p.tokens, 18, HEAD, asked).unwrap();
    assert_eq!(
        cache.builds(),
        4,
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
    let got = abandon::scope(flag, async {
        graph::cached(&p.ctx, reg, &p.tokens, 18, HEAD)
    })
    .await;
    assert_eq!(got.err().map(|e| e.code), Some(ErrorCode::Timeout));
    assert_eq!(cache.builds(), 0, "брошенный расчёт граф не строит");
    // Живой расчёт (страж держится) — граф строится; итог равен построению без обёртки.
    let (_g, flag) = abandon::guard();
    let live = abandon::scope(flag, async {
        graph::cached(&p.ctx, reg, &p.tokens, 18, HEAD)
    })
    .await
    .unwrap();
    assert_eq!(cache.builds(), 1);
    assert_eq!(
        live.len(),
        graph::build(&p.ctx, reg, &p.tokens, 18).unwrap().len()
    );
}
