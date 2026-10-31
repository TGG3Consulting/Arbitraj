//! Файл: tests/bsc_dex_cycle_exec.rs
//! Что делает: проверяет СБОРКУ атомарного бандла исполнения цикла DEX ↔ DEX (exec/cycle_bundle.rs, решения 85, 86,
//! вариант 2) на РЕАЛЬНОЙ записи маршрута CAKE 2026-10-29 (tests/route_common, routes_2026-10-29) — без сети
//! (all_approvals = true, разрешения не читаются с узла) и без моков. Пулы настоящие, на одном блоке 130521896.
//! Проверяется: (1) двухшаговый цикл USDT → CAKE → USDT двумя пулами РАЗНЫХ Universal Router — два участка (два свопа),
//! строгий порядок nonce, per-hop minOut, межроутерный переход берёт вход = гарантированному минимуму предыдущего
//! участка (Permit2 на фиксированную сумму); (2) трёхшаговый цикл USDT → CAKE → WBNB → USDT, где первый хоп на Uniswap,
//! а два следующих на одном PancakeSwap Universal Router — участок из двух хопов собирается ОДНИМ execute (через
//! CONTRACT_BALANCE), пыль оседает в промежуточной монете межроутерного перехода (CAKE); (3) НЕисполнимый цикл (хоп
//! через не-tradable семейство THENA Fusion) в бандл не собирается — отказ; (4) гейт dex_dex.execute по умолчанию ВЫКЛ
//! — исполнения нет (require_execute отказывает), включённый — пропускает.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::exec::{cycle_bundle, bundle}, quote::{route_paths, route_calc, select},
//! pools::{graph, families}, num; tests/route_common (реальная запись, настоящая SQLite).

mod route_common;

use alloy_primitives::{Address, U256, address};
use rust_decimal::Decimal;
use serde_json::json;

use arb::settings::scope::Scope;
use arb::venues::bsc_dex::exec::bundle::BundleTx;
use arb::venues::bsc_dex::exec::cycle_bundle;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::pools::{families, graph};
use arb::venues::bsc_dex::quote::route_calc::Leg;
use arb::venues::bsc_dex::quote::route_paths::{self, RoutePath};
use arb::venues::bsc_dex::quote::select::Usable;

use route_common::{CAKE, CAKE_USDT_UNI, USDT, WBNB};

const WALLET: Address = address!("0x00000000000000000000000000000000000000aa");
/// Отдельный Universal Router (и Permit2) для Uniswap — чтобы хопы Pancake и Uniswap попали в разные участки.
const UNI_ROUTER: Address = address!("0x00000000000000000000000000000000000000bb");
const FIRST_NONCE: u64 = 7;
const NOW: u64 = 1_000_000;

/// n × 10^18 (токены с 18 знаками). Ошибок нет.
fn e18(n: u64) -> U256 {
    U256::from(n) * U256::from(1_000_000_000_000_000_000u128)
}

/// Нога по шагу цикла с заданным газом (без крючка) и decimals 18 — как в tests/bsc_dex_cycle.rs: допуск задаём явно,
/// не дёргая проверки квотера; расчёт идёт по реальным записанным пулам.
fn leg_of(step: &route_paths::Step, gas: u64) -> Leg {
    Leg {
        entry: step.edge.entry.clone(),
        usable: Usable {
            gas,
            hook_pct: Decimal::ZERO,
        },
        token_in: step.token_in,
        token_out: step.token_out,
        dec_in: 18,
        dec_out: 18,
    }
}

/// Последовательность токенов цикла.
fn seq(path: &RoutePath) -> Vec<Address> {
    let mut v = vec![path.steps[0].token_in];
    v.extend(path.steps.iter().map(|s| s.token_out));
    v
}

/// Настройки сборки: проскальзывание per-hop minOut, срок и разрешения; Uniswap — отдельный Universal Router и Permit2.
async fn prepare(p: &Parts) {
    let v = Scope::Venue("bsc_dex".into());
    for (n, val) in [
        ("exec.dex_slippage_pct", json!("1")),
        ("dex.deadline_s", json!(60)),
        ("dex.approve_unlimited", json!(false)),
        ("dex.permit2_expiration_s", json!(3600)),
    ] {
        p.ctx
            .settings
            .set(n, v.clone(), Some(val), "тест")
            .await
            .unwrap();
    }
    let uni = Scope::Dex("uniswap_v3".into());
    let addr = json!("0x00000000000000000000000000000000000000bb");
    for n in ["dex.universal_router", "dex.permit2"] {
        p.ctx
            .settings
            .set(n, uni.clone(), Some(addr.clone()), "тест")
            .await
            .unwrap();
    }
}

/// Все хопы цикла — tradable через Universal Router, по записанным пулам.
async fn graph_ready() -> (graph::Graph, Parts) {
    let p = route_common::parts().await;
    prepare(&p).await;
    let g = graph::build(&p.ctx, &p.registry, &p.tokens, 18).expect("граф строится");
    (g, p)
}

#[tokio::test]
async fn two_router_cycle_crosses_by_guaranteed_min() {
    let (g, p) = graph_ready().await;
    // Двухшаговый цикл USDT → CAKE → USDT двумя пулами пары (Pancake V3 + Uniswap V3) — разные Universal Router.
    let cyc = route_paths::enumerate_cycles(&g, USDT, 2, 100)
        .into_iter()
        .next()
        .expect("есть двухшаговый цикл");
    assert_eq!(seq(&cyc), vec![USDT, CAKE, USDT]);
    let legs: Vec<Leg> = cyc.steps.iter().map(|s| leg_of(s, 150_000)).collect();

    let a = cycle_bundle::build(
        &p.ctx,
        &legs,
        Decimal::new(100, 0),
        WALLET,
        FIRST_NONCE,
        NOW,
        true,
    )
    .await
    .expect("бандл цикла собран");

    // Два участка (два разных роутера) — по свопу на участок, каждый один хоп.
    assert_eq!(a.bundle.swaps, 2, "два роутера — два свопа");
    assert_eq!(a.segments.len(), 2);
    assert_ne!(
        a.segments[0].router, a.segments[1].router,
        "участки на разных роутерах"
    );
    assert!(
        a.segments.iter().all(|s| s.hops == 1),
        "по одному хопу на участок"
    );

    // Nonce строго по очереди от FIRST_NONCE (решение 45).
    for (i, tx) in a.bundle.txs.iter().enumerate() {
        assert_eq!(tx.nonce(), FIRST_NONCE + i as u64);
    }
    // Каждый своп следует за своими разрешениями (approve), не наоборот.
    let first_swap = a.bundle.txs.iter().position(|t| t.is_swap()).unwrap();
    assert!(first_swap > 0, "перед свопом есть разрешения");
    assert!(matches!(
        a.bundle.txs[first_swap - 1],
        BundleTx::Approve { .. }
    ));

    // Вход первого участка — проба (100 базовой USDT, 18 знаков); per-hop minOut > 0.
    assert_eq!(
        a.segments[0].input_units,
        e18(100),
        "вход первого участка = проба"
    );
    for s in &a.segments {
        assert!(
            s.guaranteed_min > U256::ZERO,
            "гарантированный минимум участка > 0"
        );
    }
    let swaps: Vec<&BundleTx> = a.bundle.txs.iter().filter(|t| t.is_swap()).collect();
    for s in &swaps {
        if let BundleTx::Swap { legs_min, .. } = s {
            assert_eq!(legs_min.len(), 1, "один хоп на участок");
            assert!(legs_min[0] > U256::ZERO, "per-hop minOut > 0");
        }
    }
    // МЕЖРОУТЕРНЫЙ переход: вход второго участка = гарантированному минимуму первого (Permit2 на эту сумму).
    assert_eq!(
        a.segments[1].input_units, a.segments[0].guaranteed_min,
        "вход второго участка = minOut первого"
    );
    // Пыль оседает в промежуточной монете (CAKE) межроутерного перехода.
    assert_eq!(
        a.dust_tokens(),
        vec![CAKE],
        "пыль в промежуточной монете перехода"
    );
    // Последний участок возвращает базовую монету (BALANCE_CHECK на базу).
    assert_eq!(
        a.segments[1].out_token, USDT,
        "цикл возвращается в базовую монету"
    );
}

#[tokio::test]
async fn in_router_segment_chains_two_hops() {
    let (g, p) = graph_ready().await;
    // Трёхшаговый цикл USDT → CAKE → WBNB → USDT, первый хоп на Uniswap (CAKE-USDT UNI), следующие два — на одном
    // PancakeSwap Universal Router: участок из двух хопов собирается ОДНИМ execute (цепочка CONTRACT_BALANCE).
    let cyc = route_paths::enumerate_cycles(&g, USDT, 3, 200)
        .into_iter()
        .find(|c| {
            seq(c) == vec![USDT, CAKE, WBNB, USDT]
                && c.steps[0].edge.entry.pool.reference
                    == arb::core::types::PoolRef::Contract(CAKE_USDT_UNI)
        })
        .expect("есть цикл USDT → CAKE(uni) → WBNB → USDT");
    let legs: Vec<Leg> = cyc.steps.iter().map(|s| leg_of(s, 150_000)).collect();

    let a = cycle_bundle::build(
        &p.ctx,
        &legs,
        Decimal::new(100, 0),
        WALLET,
        FIRST_NONCE,
        NOW,
        true,
    )
    .await
    .expect("бандл цикла собран");

    // Два участка: Uniswap (1 хоп) и PancakeSwap (2 хопа в одном свопе).
    assert_eq!(a.bundle.swaps, 2, "два роутера — два свопа");
    assert_eq!(a.segments.len(), 2);
    assert_eq!(a.segments[0].hops, 1, "участок Uniswap — один хоп");
    assert_eq!(
        a.segments[1].hops, 2,
        "участок PancakeSwap — два хопа в одном execute"
    );
    assert_eq!(
        a.segments[0].router, UNI_ROUTER,
        "первый участок — Uniswap Universal Router"
    );

    // Участок из двух хопов — один своп с двумя per-hop minOut (цепочка CONTRACT_BALANCE внутри одного execute).
    let two_hop = a
        .bundle
        .txs
        .iter()
        .find_map(|t| match t {
            BundleTx::Swap { legs_min, .. } if legs_min.len() == 2 => Some(legs_min.clone()),
            _ => None,
        })
        .expect("есть своп из двух хопов");
    assert!(
        two_hop.iter().all(|m| *m > U256::ZERO),
        "per-hop minOut обоих хопов > 0"
    );

    // Межроутерный переход: вход PancakeSwap-участка = minOut Uniswap-участка; пыль — в CAKE; финиш — USDT.
    assert_eq!(a.segments[1].input_units, a.segments[0].guaranteed_min);
    assert_eq!(a.dust_tokens(), vec![CAKE]);
    assert_eq!(a.segments[1].out_token, USDT);
    // Nonce по очереди.
    for (i, tx) in a.bundle.txs.iter().enumerate() {
        assert_eq!(tx.nonce(), FIRST_NONCE + i as u64);
    }
}

#[tokio::test]
async fn non_tradable_hop_not_assembled() {
    let (g, p) = graph_ready().await;
    let cyc = route_paths::enumerate_cycles(&g, USDT, 2, 100)
        .into_iter()
        .next()
        .unwrap();
    let mut legs: Vec<Leg> = cyc.steps.iter().map(|s| leg_of(s, 150_000)).collect();
    // Подменяем семейство одного хопа на не-tradable (THENA Fusion — в графе, но авто-сделкой не исполняется).
    legs[0].entry.family = families::make("thena_fusion", "thena_fusion").unwrap();
    assert!(!legs[0].entry.family.tradable);

    let err = cycle_bundle::build(
        &p.ctx,
        &legs,
        Decimal::new(100, 0),
        WALLET,
        FIRST_NONCE,
        NOW,
        true,
    )
    .await
    .expect_err("не-tradable хоп → цикл бандлом не собирается");
    assert_eq!(err.code, arb::gateway::errors::ErrorCode::NoRoute);
    assert!(err.message.contains("thena_fusion"), "{}", err.message);
}

#[tokio::test]
async fn execute_gate_off_by_default() {
    let (_g, p) = graph_ready().await;
    // По умолчанию dex_dex.execute не задан — гейт закрыт, исполнения нет (цикл только сигнал).
    assert!(!cycle_bundle::execute_enabled(&p.ctx));
    let err = cycle_bundle::require_execute(&p.ctx).expect_err("гейт закрыт — сделки нет");
    assert_eq!(err.code, arb::gateway::errors::ErrorCode::NoRoute);
    assert!(err.message.contains("dex_dex.execute"), "{}", err.message);

    // Включённый гейт пропускает.
    p.ctx
        .settings
        .set(
            "dex_dex.execute",
            Scope::Venue("bsc_dex".into()),
            Some(json!(true)),
            "тест",
        )
        .await
        .unwrap();
    assert!(cycle_bundle::execute_enabled(&p.ctx));
    assert!(cycle_bundle::require_execute(&p.ctx).is_ok());
}
