//! Файл: tests/bsc_dex_approve_plan.rs
//! Что делает: решение 271 (дополнение, план = факт): группы разрешений котировки бандла (quote/approve_bundle.rs plan/txs)
//! совпадают с разрешениями, которые собирает сам бандл (exec/bundle.rs::build, all_approvals — без узла), на РЕАЛЬНЫХ
//! резервах записи routes_2026-10-29 (tests/route_common; пулы форка — резервы записанного V2-пула WBNB-USDT с меткой
//! семейства v2_fork, как tests/bsc_dex_fork_swap.rs; к сети тест не ходит). Сценарии:
//! (а) сплит на два разных форка (два роутера) — два approve; (в) сплит «форк + Pancake UR» — approve форка и пара
//! ERC-20 → Permit2, Permit2 → UR; (б) две доли на одном роутере форка — ОДИН approve на сумму входов обеих долей.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{exec::bundle, exec::chain, exec::plan, quote::{approve_bundle, graph_route},
//! node::abi, pools}, tests/route_common.

mod route_common;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, U256, address};
use rust_decimal::Decimal;
use serde_json::json;

use arb::core::types::{DexRoute, PoolRef, PoolState, RouteHop, RouteLeg, RouteMode, Side};
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::exec::bundle::{self, BundleTx};
use arb::venues::bsc_dex::exec::chain::Step;
use arb::venues::bsc_dex::exec::plan::Order;
use arb::venues::bsc_dex::node::abi::IERC20;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::pools::families;
use arb::venues::bsc_dex::pools::registry::Entry;
use arb::venues::bsc_dex::quote::approve_bundle;
use arb::venues::bsc_dex::quote::graph_route::{GraphForm, graph_form};

use route_common::{CAKE, CAKE_USDT_PCS, USDT, WBNB_USDT_V2};

const WALLET: Address = address!("0x00000000000000000000000000000000000000aa");
/// Роутеры двух форков сцены (dex.router; к сети не используются) и их пулы CAKE-USDT.
const BISWAP: (&str, Address, Address) = (
    "biswap",
    address!("0x00000000000000000000000000000000000000b1"),
    address!("0x00000000000000000000000000000000000000b2"),
);
const BABY: (&str, Address, Address) = (
    "babyswap",
    address!("0x00000000000000000000000000000000000000b3"),
    address!("0x00000000000000000000000000000000000000b4"),
);

fn e18(n: u64) -> U256 {
    U256::from(n) * U256::from(1_000_000_000_000_000_000u128)
}

/// Окружение записи + два форка (строки DEX, пулы CAKE-USDT в реестре на реальных резервах V2-пула записи),
/// настройки сборки бандла, бандловый канал. Ошибок нет (паника теста).
async fn scene() -> Parts {
    let p = route_common::parts().await;
    let v = Scope::Venue("bsc_dex".into());
    for (n, val) in [
        ("dex.deadline_s", json!(60)),
        ("dex.approve_unlimited", json!(false)),
        ("dex.permit2_expiration_s", json!(3600)),
        ("dex.bundle_max_routers", json!(3)),
    ] {
        p.ctx
            .settings
            .set(n, v.clone(), Some(val), "тест")
            .await
            .unwrap();
    }
    route_common::channel(&p, "club48", "bundle").await;
    let real = p.registry.get(&PoolRef::Contract(WBNB_USDT_V2)).unwrap();
    for (id, router, pool) in [BISWAP, BABY] {
        let a = json!(router.to_string());
        for (n, val) in [
            ("dex.kind", json!("v2_fork")),
            ("dex.pool_enabled", json!(true)),
            ("dex.factory", a.clone()),
            ("dex.router", a.clone()),
            ("dex.v2_fee_source", json!("fixed")),
            ("dex.v2_fee_pct", json!("0.25")),
        ] {
            let s = Scope::Dex(id.into());
            p.ctx.settings.set(n, s, Some(val), "тест").await.unwrap();
        }
        let f = families::make(id, "v2_fork").unwrap();
        let mut e: Entry = real.clone();
        assert!(
            matches!(e.pool.state, PoolState::V2 { .. }),
            "V2-пул записи"
        );
        e.family = f;
        e.pool.family = f.id.into();
        e.pool.reference = PoolRef::Contract(pool);
        (e.pool.token0, e.pool.token1) = (CAKE, USDT);
        (e.coin, e.usdt, e.ticker) = (CAKE, USDT, "CAKE".into());
        p.registry.put(e);
    }
    p
}

/// Маршрут графа (доли по одному хопу USDT → CAKE) и ноги бандла из тех же пулов. Принимает: пулы и входы долей.
fn route_and_legs(p: &Parts, legs: &[(Address, u64)]) -> (DexRoute, Vec<Vec<Step>>) {
    let total: u64 = legs.iter().map(|l| l.1).sum();
    let mut r = DexRoute {
        mode: RouteMode::Graph,
        legs: Vec::new(),
        block: 1,
        gas_units: 0,
        executable: true,
        exec_note: None,
    };
    let mut steps = Vec::new();
    for &(pool, amount) in legs {
        let e = p
            .registry
            .get(&PoolRef::Contract(pool))
            .expect("пул в реестре");
        r.legs.push(RouteLeg {
            share: Decimal::from(amount) / Decimal::from(total),
            hops: vec![RouteHop {
                family: e.family.id.into(),
                kind: e.pool.kind,
                pool: e.pool.reference.clone(),
            }],
            expected_out: Decimal::ONE,
        });
        let order = Order {
            token_in: USDT,
            token_out: CAKE,
            amount_in: e18(amount),
            min_out: e18(1),
        };
        steps.push(vec![Step {
            entry: e,
            order,
            from_wallet: true,
            to_wallet: true,
        }]);
    }
    (r, steps)
}

/// Разрешения котировки и бандла для маршрута: (группы плана, транзакции Approve бандла). Ошибок нет (паника).
async fn both(p: &Parts, legs: &[(Address, u64)]) -> (Vec<approve_bundle::Unit>, Vec<BundleTx>) {
    let (route, steps) = route_and_legs(p, legs);
    let q = p.quoting();
    assert_eq!(
        graph_form(&q.ctx, &route),
        GraphForm::Bundle,
        "форма «бандл»"
    );
    let units = approve_bundle::plan(&q, &route, Side::Buy).expect("план разрешений");
    let b = bundle::build(&q.ctx, &steps, CAKE, WALLET, 7, 1_000_000, true)
        .await
        .expect("бандл собран");
    let appr = b.txs.into_iter().filter(|t| !t.is_swap()).collect();
    (units, appr)
}

/// (а) Два разных форка: в котировке две группы (по approve), бандл шлёт два approve.
#[tokio::test]
async fn two_forks_two_approvals() {
    let p = scene().await;
    let (units, appr) = both(&p, &[(BISWAP.2, 600), (BABY.2, 400)]).await;
    assert_eq!(units.len(), 2, "{units:?}");
    assert_eq!(approve_bundle::txs(&units), appr.len(), "план = факт");
    assert_eq!(appr.len(), 2);
}

/// (в) Форк + Pancake UR: approve форка + ERC-20 → Permit2 и Permit2 → UR — три транзакции и в плане, и в бандле.
#[tokio::test]
async fn fork_plus_ur_counts_permit2() {
    let p = scene().await;
    let (units, appr) = both(&p, &[(BISWAP.2, 700), (CAKE_USDT_PCS, 300)]).await;
    assert_eq!(units.len(), 2, "{units:?}");
    assert!(
        units.iter().any(|u| !u.fork && u.permit2.is_some()),
        "группа UR с Permit2"
    );
    assert_eq!(approve_bundle::txs(&units), appr.len(), "план = факт");
    assert_eq!(appr.len(), 3);
}

/// (б) Две доли на одном роутере форка: один approve на сумму входов (1000 + 500), в котировке одна группа.
#[tokio::test]
async fn same_fork_router_one_approval_on_sum() {
    let p = scene().await;
    let (units, appr) = both(&p, &[(BISWAP.2, 1000), (BISWAP.2, 500)]).await;
    assert_eq!(units.len(), 1, "{units:?}");
    assert_eq!(units[0].share, Decimal::ONE);
    assert_eq!(approve_bundle::txs(&units), appr.len(), "план = факт");
    let BundleTx::Approve { to, data, .. } = &appr[0] else {
        panic!("approve")
    };
    assert_eq!(*to, USDT, "approve токена входа");
    let call = IERC20::approveCall::abi_decode(data).expect("ERC-20 approve");
    assert_eq!(call.spender, BISWAP.1, "роутеру форка");
    assert_eq!(call.value, e18(1500), "на сумму входов обеих долей");
}

/// На блоке Ethereum 26494988 увеличение ненулевого разрешения Tether
/// вернуло revert. При повторе после уже включённого approve(0) нужен только
/// следующий вызов. Данные RPC и исходная транзакция записаны в fixture.
#[tokio::test]
async fn ethereum_usdt_reset_and_retry_use_observed_allowance() {
    use arb::venues::bsc_dex::wallet::approve::{Read, lacking};
    let capture: serde_json::Value =
        serde_json::from_str(include_str!("data/ethereum_usdt_approval.json")).unwrap();
    let token: Address = capture["contract"].as_str().unwrap().parse().unwrap();
    let spender: Address = capture["spender_public_from_approval_event"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let needed = U256::from_str_radix(capture["replacement_units"].as_str().unwrap(), 10).unwrap();
    let previous = U256::from_str_radix(capture["current_units"].as_str().unwrap(), 10).unwrap();
    assert_eq!(capture["approve_increase"]["error"]["code"], 3);
    let p = scene().await;
    p.ctx
        .settings
        .set(
            "dex.approve_reset_tokens",
            Scope::Venue("bsc_dex".into()),
            Some(json!([token])),
            "тест",
        )
        .await
        .unwrap();
    for (erc20, expected) in [
        (previous, vec![U256::ZERO, needed]),
        (U256::ZERO, vec![needed]),
        (needed, vec![]),
    ] {
        let read = Read {
            erc20,
            permit2: needed,
            expiration: 3600,
        };
        let calls = lacking(&p.ctx, &read, (token, spender, spender), needed, 10).unwrap();
        let amounts: Vec<U256> = calls
            .into_iter()
            .map(|(to, data)| {
                assert_eq!(to, token);
                let a = arb::venues::bsc_dex::node::abi::IERC20::approveCall::abi_decode(&data)
                    .unwrap();
                assert_eq!(a.spender, spender);
                a.value
            })
            .collect();
        assert_eq!(amounts, expected);
    }
}
