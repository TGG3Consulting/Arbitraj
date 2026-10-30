//! Файл: tests/bsc_dex_bundle.rs
//! Что делает: сборка бандла свопов мульти-роутерного графового маршрута (exec/bundle.rs, решение 84, фаза 80.8a) на
//! пулах реальной записи CAKE (tests/route_common, routes_2026-10-29): ноги группируются по Universal Router — на
//! каждый роутер один своп-execute; разрешения отдельными транзакциями перед свопами; nonce строго по очереди
//! (решение 45); minOut каждой ноги (per-leg) сохранён. Без реальной сети (all_approvals = true — разрешения не
//! читаются с узла). Проверяет структуру бандла, порядок, per-leg minOut, слияние ног одного роутера и отказ на ноге,
//! пересекающей роутеры (v1).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::exec::{bundle, chain, plan}, pools::registry, tests/route_common.

mod route_common;

use alloy_primitives::{Address, U256, address};
use serde_json::json;

use arb::core::types::PoolRef;
use arb::gateway::errors::ErrorCode;
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::exec::bundle::{self, BundleTx};
use arb::venues::bsc_dex::exec::chain::Step;
use arb::venues::bsc_dex::exec::plan::Order;
use arb::venues::bsc_dex::pools::registry::Entry;

use route_common::{CAKE, CAKE_USDT_PCS, CAKE_USDT_UNI, CAKE_WBNB_PCS, USDT, WBNB, WBNB_USDT_V2};

const WALLET: Address = address!("0x00000000000000000000000000000000000000aa");
/// Отдельный Universal Router (и Permit2) для Uniswap в тесте — чтобы ноги попали в разные группы.
const UNI_ROUTER: Address = address!("0x00000000000000000000000000000000000000bb");
const FIRST_NONCE: u64 = 5;
const NOW: u64 = 1_000_000;

/// n × 10^18 в U256 (токены с 18 знаками). Ошибок нет.
fn e18(n: u64) -> U256 {
    U256::from(n) * U256::from(1_000_000_000_000_000_000u128)
}

fn entry(q: &arb::venues::bsc_dex::quote::Quoting, addr: Address) -> Entry {
    q.reg.get(&PoolRef::Contract(addr)).expect("пул в реестре")
}

#[allow(clippy::too_many_arguments)]
fn step(
    e: Entry,
    token_in: Address,
    token_out: Address,
    amount_in: U256,
    min_out: U256,
    from_wallet: bool,
    to_wallet: bool,
) -> Step {
    Step {
        entry: e,
        order: Order {
            token_in,
            token_out,
            amount_in,
            min_out,
        },
        from_wallet,
        to_wallet,
    }
}

/// Задаёт настройки, нужные сборке бандла (срок, разрешения) и разные роутеры для Pancake и Uniswap.
async fn prepare(p: &arb::venues::bsc_dex::parts::Parts, split_routers: bool) {
    let v = Scope::Venue("bsc_dex".into());
    for (n, val) in [
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
    if split_routers {
        let d = Scope::Dex("uniswap_v3".into());
        let uni = json!("0x00000000000000000000000000000000000000bb");
        for n in ["dex.universal_router", "dex.permit2"] {
            p.ctx
                .settings
                .set(n, d.clone(), Some(uni.clone()), "тест")
                .await
                .unwrap();
        }
    }
}

#[tokio::test]
async fn two_routers_make_two_swaps() {
    // Две доли покупки CAKE: одна на Pancake (CAKE-USDT V3), одна на Uniswap (CAKE-USDT V3) — разные Universal Router.
    let p = route_common::parts().await;
    prepare(&p, true).await;
    let q = p.quoting();
    let leg_pcs = vec![step(
        entry(&q, CAKE_USDT_PCS),
        USDT,
        CAKE,
        e18(1000),
        e18(490),
        true,
        true,
    )];
    let leg_uni = vec![step(
        entry(&q, CAKE_USDT_UNI),
        USDT,
        CAKE,
        e18(1000),
        e18(495),
        true,
        true,
    )];
    let b = bundle::build(
        &q.ctx,
        &[leg_pcs, leg_uni],
        CAKE,
        WALLET,
        FIRST_NONCE,
        NOW,
        true,
    )
    .await
    .expect("бандл собран");
    assert_eq!(b.swaps, 2, "по свопу на роутер");
    // Nonce строго по очереди от FIRST_NONCE (решение 45).
    for (i, tx) in b.txs.iter().enumerate() {
        assert_eq!(tx.nonce(), FIRST_NONCE + i as u64);
    }
    // Разрешения идут перед своим свопом; свопов ровно два, с per-leg minOut и разными роутерами.
    let swaps: Vec<&BundleTx> = b.txs.iter().filter(|t| t.is_swap()).collect();
    assert_eq!(swaps.len(), 2);
    let mut routers = Vec::new();
    for s in &swaps {
        if let BundleTx::Swap { plan, legs_min, .. } = s {
            assert_eq!(legs_min.len(), 1, "одна нога на роутер");
            routers.push(plan.router);
        }
    }
    let pcs_min = match swaps[0] {
        BundleTx::Swap { legs_min, .. } => legs_min[0],
        _ => unreachable!(),
    };
    let uni_min = match swaps[1] {
        BundleTx::Swap { legs_min, .. } => legs_min[0],
        _ => unreachable!(),
    };
    assert_eq!(pcs_min, e18(490), "per-leg minOut Pancake");
    assert_eq!(uni_min, e18(495), "per-leg minOut Uniswap");
    assert_ne!(routers[0], routers[1], "свопы на разные Universal Router");
    assert_eq!(routers[1], UNI_ROUTER);
    // Каждый своп следует за своими разрешениями (approve), не наоборот.
    let first_swap = b.txs.iter().position(|t| t.is_swap()).unwrap();
    assert!(first_swap > 0, "перед свопом есть разрешения");
    assert!(matches!(b.txs[first_swap - 1], BundleTx::Approve { .. }));
}

#[tokio::test]
async fn same_router_merges_into_one_swap() {
    // Две доли Pancake (прямая CAKE-USDT V3 и двуххоповая USDT→WBNB→CAKE) — один роутер → один execute с двумя ногами.
    let p = route_common::parts().await;
    prepare(&p, false).await;
    let q = p.quoting();
    let leg_direct = vec![step(
        entry(&q, CAKE_USDT_PCS),
        USDT,
        CAKE,
        e18(1000),
        e18(490),
        true,
        true,
    )];
    let leg_hop = vec![
        step(
            entry(&q, WBNB_USDT_V2),
            USDT,
            WBNB,
            e18(1000),
            e18(2),
            true,
            false,
        ),
        step(
            entry(&q, CAKE_WBNB_PCS),
            WBNB,
            CAKE,
            e18(2),
            e18(480),
            false,
            true,
        ),
    ];
    let b = bundle::build(
        &q.ctx,
        &[leg_direct, leg_hop],
        CAKE,
        WALLET,
        FIRST_NONCE,
        NOW,
        true,
    )
    .await
    .expect("бандл собран");
    assert_eq!(b.swaps, 1, "один роутер — один своп");
    let swap = b.txs.iter().find(|t| t.is_swap()).expect("есть своп");
    if let BundleTx::Swap {
        legs_min, nonce, ..
    } = swap
    {
        assert_eq!(legs_min.len(), 2, "две ноги в одном свопе");
        assert_eq!(
            *legs_min,
            vec![e18(490), e18(480)],
            "per-leg minOut обеих ног"
        );
        // Разрешения (2: erc20 + permit2) идут первыми, своп — последним.
        assert_eq!(*nonce, FIRST_NONCE + 2);
    } else {
        unreachable!()
    }
    assert_eq!(b.txs.len(), 3, "2 разрешения + 1 своп");
}

#[tokio::test]
async fn leg_crossing_routers_is_rejected() {
    // Нога из двух шагов на разных роутерах (Uniswap, затем Pancake) — в v1 не собирается (нужна межроутерная передача).
    let p = route_common::parts().await;
    prepare(&p, true).await;
    let q = p.quoting();
    let leg = vec![
        step(
            entry(&q, CAKE_USDT_UNI),
            USDT,
            WBNB,
            e18(1000),
            e18(2),
            true,
            false,
        ),
        step(
            entry(&q, CAKE_WBNB_PCS),
            WBNB,
            CAKE,
            e18(2),
            e18(480),
            false,
            true,
        ),
    ];
    let err = bundle::build(&q.ctx, &[leg], CAKE, WALLET, FIRST_NONCE, NOW, true)
        .await
        .expect_err("нога пересекает роутеры — отказ");
    assert_eq!(err.code, ErrorCode::NoRoute);
    assert!(err.message.contains("пересекает"), "{}", err.message);
}
