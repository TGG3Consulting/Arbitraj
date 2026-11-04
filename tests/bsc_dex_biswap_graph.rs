//! Файл: tests/bsc_dex_biswap_graph.rs
//! Что делает: проверяет разделение graphable/tradable/fork для V2-форков (решения 82, 59, фазы 80.9a/9b) на РЕАЛЬНЫХ
//! резервах записи routes_2026-10-29 (единственный V2-пул записи — WBNB-USDT v2): меняется только МЕТКА семейства пула
//! (v2_fork вместо pancakeswap_v2), резервы реальные, к сети тест не ходит. Утверждает: (1) форк-пул входит в граф
//! (family.graphable); (2) после флипа 80.9b форк-маршрут ИСПОЛНИМ (DexRoute.executable) и ведётся бандлом
//! (GraphForm::Bundle), а без dex.bundle_max_routers или при включённом не-бандловом канале (решение 121 B3) —
//! несобираем (сигнал); (3) не-форк исполним прямой формой (Direct);
//! (4) решение 121: неисполнимый пул (Solidly, tradable = нет) не выигрывает классическую котировку как Direct, а
//! виден как signal_route; только неисполнимый — NoRoute «только сигнал» (quote/classic_direct.rs);
//! плюс флаги make()/classify() форка (graphable+tradable+fork; фабрика/роутер/комиссия нужны, UR/Permit2 — нет).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{pools::{families,graph,registry}, quote::{graph_route,steps,via}, parts},
//! tests/route_common (реальная запись и окружение).

mod route_common;

use alloy_primitives::{Address, U256, address};
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::json;

use arb::core::types::{
    Asset, CheckState, CoinCheck, DexRoute, Pool, PoolRef, PoolState, Side, TokenTaxes, VenueId,
};
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::pools::registry::Entry;
use arb::venues::bsc_dex::pools::{families, graph};
use arb::venues::bsc_dex::quote::direct_route;
use arb::venues::bsc_dex::quote::graph_route::{self, GraphForm};
use arb::venues::bsc_dex::quote::steps::Names;
use arb::venues::bsc_dex::quote::via::Via;
use route_common::USDT;

/// Синтетическая монета теста (чтобы путь COIN↔USDT был только через наш один пул — детерминизм).
const COIN: Address = address!("0x00000000000000000000000000000000000000c1");
/// Адрес пула теста.
const POOL: Address = address!("0x00000000000000000000000000000000000000c2");

/// Нулевые налоги.
fn zero() -> TokenTaxes {
    TokenTaxes {
        buy_pct: Decimal::ZERO,
        sell_pct: Decimal::ZERO,
        transfer_pct: Decimal::ZERO,
    }
}

/// Пул COIN-USDT вида `kind` (id DEX = `id`) с реальными резервами записи. token0 < token1 по адресу.
fn coin_usdt(id: &str, kind: &str, r0: U256, r1: U256) -> Entry {
    let f = families::make(id, kind).unwrap();
    let (t0, t1) = if COIN < USDT {
        (COIN, USDT)
    } else {
        (USDT, COIN)
    };
    Entry {
        family: f,
        pool: Pool {
            venue: VenueId("bsc_dex".into()),
            family: f.id.into(),
            kind: f.kind,
            reference: PoolRef::Contract(POOL),
            token0: t0,
            token1: t1,
            fee_pips: Some(2500),
            tick_spacing: None,
            bin_step: None,
            hooks: None,
            state: PoolState::V2 {
                reserve0: r0,
                reserve1: r1,
            },
            block: 1,
            received_at: Utc::now(),
        },
        ticker: "COIN".into(),
        coin: COIN,
        usdt: USDT,
        key: None,
        ticks: None,
        bins: None,
        synced_block: 1,
        synced_log: None,
        valid: true,
        asleep: false,
    }
}

/// Реальные резервы единственного V2-пула записи routes_2026-10-29 (WBNB-USDT v2). Ошибок нет (в записи он есть).
fn real_v2_reserves(p: &Parts) -> (U256, U256) {
    p.registry
        .all()
        .into_iter()
        .find_map(|e| match e.pool.state {
            PoolState::V2 { reserve0, reserve1 } => Some((reserve0, reserve1)),
            _ => None,
        })
        .expect("V2-пул в записи routes_2026-10-29")
}

/// Окружение записи с одним пулом COIN-USDT заданного семейства (реальные резервы), допущенным к котировке
/// (газ + сверка с квотером по обеим сторонам) и с известными нулевыми налогами COIN.
async fn scene(id: &str, kind: &str) -> Parts {
    let p = route_common::parts().await;
    let (r0, r1) = real_v2_reserves(&p);
    // Строка настроек DEX форка: фабрика/роутер/комиссия — БЕЗ universal_router/permit2 (форку не нужны).
    let s = Scope::Dex(id.into());
    let a = json!("0x0000000000000000000000000000000000000099");
    for (n, v) in [
        ("dex.kind", json!(kind)),
        ("dex.pool_enabled", json!(true)),
        ("dex.factory", a.clone()),
        ("dex.router", a.clone()),
        ("dex.v2_fee_source", json!("fixed")),
        ("dex.v2_fee_pct", json!("0.25")),
    ] {
        p.ctx
            .settings
            .set(n, s.clone(), Some(v), "тест")
            .await
            .unwrap();
    }
    // Решение 121 B3: бандл-маршрут исполним, только если все включённые каналы бандловые — один такой канал.
    route_common::channel(&p, "club48", "bundle").await;
    // Лимит роутеров бандла — чтобы форк-маршрут собирался (collectable): 1 роутер форка ≤ 3.
    p.ctx
        .settings
        .set(
            "dex.bundle_max_routers",
            Scope::Venue("bsc_dex".into()),
            Some(json!(3)),
            "тест",
        )
        .await
        .unwrap();
    p.tokens.set(&[
        Asset {
            ticker: "USDT".into(),
            bsc_contract: Some(USDT),
        },
        Asset {
            ticker: "COIN".into(),
            bsc_contract: Some(COIN),
        },
    ]);
    p.tokens.remember_decimals(USDT, 18);
    p.tokens.remember_decimals(COIN, 18);
    p.checks.set_coin(CoinCheck {
        venue: VenueId("bsc_dex".into()),
        asset: Asset {
            ticker: "COIN".into(),
            bsc_contract: Some(COIN),
        },
        taxes: CheckState::Known(zero()),
        source: "тест".into(),
        checked_at: Some(Utc::now()),
        wait: None,
    });
    p.registry.put(coin_usdt(id, kind, r0, r1));
    p.checks.update_pool(&PoolRef::Contract(POOL), |f| {
        f.gas_buy = Some(120_000);
        f.gas_sell = Some(120_000);
        f.quoter.buy = Some(Ok(()));
        f.quoter.sell = Some(Ok(()));
        f.taxes = Some(route_common::clean_taxes());
    });
    p
}

/// Котирует покупку COIN графом и возвращает маршрут (с флагом executable и exec_note). Ошибок нет.
async fn quote_route(p: &Parts) -> DexRoute {
    let q = p.quoting();
    let taxes = zero();
    let v = Via {
        q: &q,
        coin: COIN,
        usdt: USDT,
        side: Side::Buy,
        taxes: &taxes,
        coin_dec: 18,
        usdt_dec: 18,
    };
    let names = Names {
        usdt: (USDT, "USDT"),
        coin: (COIN, "COIN"),
        mids: &p.mids,
    };
    let asset = Asset {
        ticker: "COIN".into(),
        bsc_contract: Some(COIN),
    };
    let quote = graph_route::quote(
        &v,
        &names,
        &asset,
        Decimal::from(1000u32),
        U256::from(2_000_000u64),
    )
    .await
    .expect("котировка графа");
    quote.route.expect("маршрут графа")
}

/// make(): v2_fork — graphable, tradable и fork (исполним своим роутером, 80.9b); поддержанный V2 — graphable+tradable,
/// НЕ форк; v3_fork (решение 122 F) — в графе и сигналах, но без своего пути исполнения (graphable, НЕ tradable; шаг V3
/// контракта-исполнителя).
#[test]
fn family_flags_split() {
    let fork = families::make("biswap", "v2_fork").unwrap();
    assert!(
        fork.graphable && fork.tradable && fork.fork,
        "v2_fork — в графе, исполним и форк"
    );
    let classic = families::make("pancakeswap_v2", "pancakeswap_v2").unwrap();
    assert!(
        classic.graphable && classic.tradable && !classic.fork,
        "поддержанный V2 — graphable+tradable, не форк"
    );
    let v3f = families::make("some-v3-fork", "v3_fork").unwrap();
    assert!(
        v3f.graphable && !v3f.tradable && v3f.fork,
        "v3_fork — в графе, сигнал, исполнение через исполнитель"
    );
}

/// classify(): V2-форк подключается в граф с фабрикой/роутером/комиссией БЕЗ Universal Router и Permit2; без роутера —
/// не подключается; v3_fork без dex.pool_enabled и адресов — не подключается.
#[tokio::test]
async fn classify_fork_router_not_ur() {
    let p = route_common::parts().await;
    let a = json!("0x0000000000000000000000000000000000000099");
    let row = |with_router: bool| {
        let mut v = vec![
            ("dex.kind", json!("v2_fork")),
            ("dex.pool_enabled", json!(true)),
            ("dex.factory", a.clone()),
            ("dex.v2_fee_source", json!("fixed")),
            ("dex.v2_fee_pct", json!("0.25")),
        ];
        if with_router {
            v.push(("dex.router", a.clone()));
        }
        v
    };
    for (n, v) in row(true) {
        p.ctx
            .settings
            .set(n, Scope::Dex("biswap".into()), Some(v), "тест")
            .await
            .unwrap();
    }
    let f = families::classify(&p.ctx, "biswap").expect("форк подключается без UR/Permit2");
    assert!(
        f.graphable && f.tradable && f.fork,
        "форк: в графе, исполним своим роутером"
    );
    // Без dex.router — форк не подключается (роутер нужен для сверки/исполнения позже).
    for (n, v) in row(false) {
        p.ctx
            .settings
            .set(n, Scope::Dex("biswap2".into()), Some(v), "тест")
            .await
            .unwrap();
    }
    assert!(
        families::classify(&p.ctx, "biswap2").is_err(),
        "без dex.router форк не должен подключаться"
    );
    // v3_fork (решение 122 F — graphable) без dex.pool_enabled/фабрики/TickLens/квотера не подключается.
    p.ctx
        .settings
        .set(
            "dex.kind",
            Scope::Dex("v3f".into()),
            Some(json!("v3_fork")),
            "тест",
        )
        .await
        .unwrap();
    assert!(
        families::classify(&p.ctx, "v3f").is_err(),
        "v3_fork без настроек не подключается"
    );
}

/// Форк-пул входит в граф (graphable+fork).
#[tokio::test]
async fn fork_pool_enters_graph() {
    let p = scene("biswap", "v2_fork").await;
    let g = graph::build(&p.ctx, &p.registry, &p.tokens, 18).expect("граф строится");
    let edge = g
        .between(COIN, USDT)
        .into_iter()
        .find(|e| e.entry.pool.reference == PoolRef::Contract(POOL))
        .expect("форк-ребро COIN-USDT должно быть в графе (graphable)");
    assert!(
        edge.entry.family.graphable && edge.entry.family.fork,
        "ребро — форк в графе"
    );
}

/// Флип 80.9b-ii: форк-маршрут теперь ИСПОЛНИМ (executable=true, причины нет) и ведётся бандлом (GraphForm::Bundle),
/// а не прямым/умным путём UR — collectable пускает его (1 роутер форка ≤ dex.bundle_max_routers).
#[tokio::test]
async fn fork_route_executable_as_bundle() {
    let p = scene("biswap", "v2_fork").await;
    let r = quote_route(&p).await;
    assert!(r.executable, "форк-маршрут теперь исполним (флип 80.9b-ii)");
    assert!(
        r.exec_note.is_none(),
        "у исполнимого нет причины: {:?}",
        r.exec_note
    );
    assert_eq!(
        graph_route::graph_form(&p.ctx, &r),
        GraphForm::Bundle,
        "форк-маршрут идёт бандлом (свой роутер), не прямым/умным путём UR"
    );
}

/// Решение 121 B3: включён ещё и не-бандловый канал — форк-маршрут (только бандлом) неисполним уже на котировке, с
/// понятной причиной (сигнал есть, сделки нет); выключили его — снова исполним.
#[tokio::test]
async fn fork_route_not_executable_with_single_channel() {
    let p = scene("biswap", "v2_fork").await;
    route_common::channel(&p, "jetbuilder", "single").await;
    let r = quote_route(&p).await;
    assert!(!r.executable, "не-бандловый канал — бандл не отправить");
    let note = r.exec_note.unwrap_or_default();
    assert!(
        note.contains("не в режиме бандла") && note.contains("JetBuilder"),
        "причина называет канал: {note}"
    );
    p.ctx
        .settings
        .set(
            "channel.enabled",
            Scope::Channel("jetbuilder".into()),
            Some(json!(false)),
            "тест",
        )
        .await
        .unwrap();
    assert!(
        quote_route(&p).await.executable,
        "только бандловые каналы — исполним"
    );
}

/// Без dex.bundle_max_routers форк-маршрут несобираем → сигнал (executable=false): collectable требует настройку.
#[tokio::test]
async fn fork_route_not_executable_without_bundle_limit() {
    let p = scene("biswap", "v2_fork").await;
    // Снимаем лимит роутеров бандла — collectable больше не пускает.
    p.ctx
        .settings
        .set(
            "dex.bundle_max_routers",
            Scope::Venue("bsc_dex".into()),
            None,
            "тест",
        )
        .await
        .unwrap();
    let r = quote_route(&p).await;
    assert!(
        !r.executable,
        "без dex.bundle_max_routers форк не собирается в бандл"
    );
    assert!(
        r.exec_note
            .is_some_and(|n| n.contains("dex.bundle_max_routers")),
        "причина — не задана настройка лимита"
    );
}

/// Ревью #86: форк НЕ становится кандидатом классического ПРЯМОГО пути, хотя проба форка сделала его usable. Без
/// фильтра (как до фикса) форк прошёл бы в direct_route и упал бы на plan::build (нет UR/Permit2); продакшн-фильтр
/// (keep |e| !e.family.fork) его исключает. Не-форк тем же путём проходит как прежде.
#[tokio::test]
async fn fork_excluded_from_classic_direct() {
    let p = scene("biswap", "v2_fork").await;
    let q = p.quoting();
    let taxes = zero();
    let v = Via {
        q: &q,
        coin: COIN,
        usdt: USDT,
        side: Side::Buy,
        taxes: &taxes,
        coin_dec: 18,
        usdt_dec: 18,
    };
    let names = Names {
        usdt: (USDT, "USDT"),
        coin: (COIN, "COIN"),
        mids: &p.mids,
    };
    let asset = Asset {
        ticker: "COIN".into(),
        bsc_contract: Some(COIN),
    };
    let fork = q
        .reg
        .get(&PoolRef::Contract(POOL))
        .expect("форк-пул в реестре");
    let qty = Decimal::from(1000u32);
    let gas = U256::from(2_000_000u64);
    // Демонстрация утечки: без фильтра проба сделала форк usable — он кандидат прямого пути.
    let leak = direct_route::quote(
        &v,
        &names,
        &asset,
        qty,
        vec![fork.clone()],
        &[],
        &|_| true,
        gas,
    )
    .await;
    assert!(
        leak.is_ok(),
        "без фильтра форк-пул прошёл бы в классический прямой путь (проба сделала его usable)"
    );
    // Фикс: продакшн-фильтр исключает форк — кандидатов нет, прямого маршрута через форк не возникает.
    let filtered = direct_route::quote(
        &v,
        &names,
        &asset,
        qty,
        vec![fork],
        &[],
        &|e| !e.family.fork,
        gas,
    )
    .await;
    assert!(
        filtered.is_err(),
        "форк исключён из классического прямого пути (исполним только graph→bundle)"
    );
    // Не-форк пул тем же путём с тем же фильтром — проходит (классика по не-форкам не сломана).
    let classic = coin_usdt(
        "pancakeswap_v2",
        "pancakeswap_v2",
        real_v2_reserves(&p).0,
        real_v2_reserves(&p).1,
    );
    let ok = direct_route::quote(
        &v,
        &names,
        &asset,
        qty,
        vec![classic],
        &[],
        &|e| !e.family.fork,
        gas,
    )
    .await;
    assert!(
        ok.is_ok(),
        "не-форк пул проходит классический прямой путь как прежде"
    );
}

/// Тот же пул как поддержанный V2 (не форк) исполним как прежде прямой формой (GraphForm::Direct).
#[tokio::test]
async fn classic_route_executable_direct() {
    let p = scene("pancakeswap_v2", "pancakeswap_v2").await;
    let r = quote_route(&p).await;
    assert!(
        r.executable,
        "не-форк одно-пуловый маршрут исполним как прежде"
    );
    assert!(
        r.exec_note.is_none(),
        "у исполнимого причины нет: {:?}",
        r.exec_note
    );
    assert_eq!(
        graph_route::graph_form(&p.ctx, &r),
        GraphForm::Direct,
        "классический одно-пуловый маршрут — прямая форма (UR)"
    );
}

/// Второй пул теста — неисполнимое семейство (Solidly, tradable = нет) рядом с исполнимым.
const POOL2: Address = address!("0x00000000000000000000000000000000000000c3");

/// Решение 121: окружение с исполнимым пулом PancakeSwap V2 (POOL) и неисполнимым Solidly-волатильным (POOL2) на тех
/// же реальных резервах записи; у Solidly комиссия ниже (1/10000 против 0.25%) — его цена лучше. Ошибок нет.
async fn scene_with_signal_only() -> Parts {
    let p = scene("pancakeswap_v2", "pancakeswap_v2").await;
    let (r0, r1) = real_v2_reserves(&p);
    let s = Scope::Dex("topaz_v2".into());
    for (n, v) in [
        ("dex.kind", json!("solidly_v2")),
        ("dex.pool_enabled", json!(true)),
        (
            "dex.factory",
            json!("0x0000000000000000000000000000000000000098"),
        ),
    ] {
        p.ctx
            .settings
            .set(n, s.clone(), Some(v), "тест")
            .await
            .unwrap();
    }
    let mut e = coin_usdt("topaz_v2", "solidly_v2", r0, r1);
    e.pool.reference = PoolRef::Contract(POOL2);
    let unit = U256::from(10u64).pow(U256::from(18u64));
    e.pool.state = PoolState::Solidly {
        reserve0: r0,
        reserve1: r1,
        stable: false,
        decimals0: unit,
        decimals1: unit,
        fee: 1,
    };
    p.registry.put(e);
    p.checks.update_pool(&PoolRef::Contract(POOL2), |f| {
        f.gas_buy = Some(120_000);
        f.gas_sell = Some(120_000);
        f.quoter.buy = Some(Ok(()));
        f.quoter.sell = Some(Ok(()));
        f.taxes = Some(route_common::clean_taxes());
    });
    p
}

/// Классическая прямая котировка покупки COIN по пулам `entries` (quote/classic_direct.rs). Ошибок нет — итог как есть.
async fn classic(
    p: &Parts,
    entries: Vec<Entry>,
) -> arb::gateway::errors::GatewayResult<arb::core::types::Quote> {
    let q = p.quoting();
    let taxes = zero();
    let v = Via {
        q: &q,
        coin: COIN,
        usdt: USDT,
        side: Side::Buy,
        taxes: &taxes,
        coin_dec: 18,
        usdt_dec: 18,
    };
    let names = Names {
        usdt: (USDT, "USDT"),
        coin: (COIN, "COIN"),
        mids: &p.mids,
    };
    let asset = Asset {
        ticker: "COIN".into(),
        bsc_contract: Some(COIN),
    };
    let gas = U256::from(2_000_000u64);
    arb::venues::bsc_dex::quote::classic_direct::quote(
        &v,
        &names,
        &asset,
        Decimal::from(1000u32),
        entries,
        &[],
        &|_| true,
        gas,
    )
    .await
}

/// Решение 121: неисполнимый пул (tradable = нет) не выигрывает классическую котировку как Direct, даже если его цена
/// лучше: маршрут сделки — исполнимый Pancake, а неисполнимый виден сигналу как signal_route (executable = нет).
#[tokio::test]
async fn signal_only_pool_not_direct_but_signal() {
    let p = scene_with_signal_only().await;
    let all = p.registry.by_pair(COIN, USDT);
    assert_eq!(all.len(), 2, "оба пула в реестре");
    let q = classic(&p, all).await.expect("исполнимая котировка есть");
    let r = q.route.as_ref().expect("маршрут");
    assert!(r.executable);
    assert!(
        r.legs[0]
            .hops
            .iter()
            .all(|h| h.pool == PoolRef::Contract(POOL)),
        "сделка — только по исполнимому пулу"
    );
    let sig = q
        .signal_route
        .as_ref()
        .expect("неисполнимый лучший пул — сигнал");
    assert!(!sig.executable);
    assert!(
        sig.legs[0]
            .hops
            .iter()
            .any(|h| h.pool == PoolRef::Contract(POOL2))
    );
    assert!(sig.exec_note.as_deref().unwrap_or("").contains("topaz_v2"));
}

/// Решение 121: у монеты только неисполнимый пул — котировки на сделку нет (NoRoute), причина «только сигнал».
#[tokio::test]
async fn only_signal_pool_is_no_route() {
    let p = scene_with_signal_only().await;
    let only: Vec<Entry> = p
        .registry
        .by_pair(COIN, USDT)
        .into_iter()
        .filter(|e| e.pool.reference == PoolRef::Contract(POOL2))
        .collect();
    let err = classic(&p, only).await.expect_err("исполнимого пула нет");
    assert_eq!(err.code, arb::gateway::errors::ErrorCode::NoRoute);
    assert!(err.message.contains("только сигнал"), "{}", err.message);
}
