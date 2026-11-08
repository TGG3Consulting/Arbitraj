//! Файл: tests/bsc_dex_executor.rs
//! Что делает: сборка вызова execute своего исполнителя ArbExecutor (exec/executor/call.rs, решение 122, часть X) из
//! долей маршрута на пулах реальной записи CAKE (tests/route_common, routes_2026-10-29): вид шага по семейству пула
//! (PancakeSwap V3 → V3, PancakeSwap V2 → V2 с комиссией пары), доли входа в bps от ТЕКУЩЕГО остатка (последняя доля —
//! весь остаток), промежуточный шаг — весь промежуточный токен, вход и минимум сделки — суммы по долям, срок —
//! время блока + dex.deadline_s; данные транзакции разбираются ABI контракта обратно в тот же вызов. Без сети.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::exec::{executor, chain, plan}, tests/route_common.

mod route_common;

use alloy::sol_types::{SolCall, SolValue};
use alloy_primitives::{Address, U256, address};
use serde_json::json;

use arb::core::types::PoolRef;
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::exec::chain::Step;
use arb::venues::bsc_dex::exec::executor::abi::ArbExecutor;
use arb::venues::bsc_dex::exec::executor::call;
use arb::venues::bsc_dex::exec::plan::Order;
use arb::venues::bsc_dex::pools::registry::Entry;

use route_common::{CAKE, CAKE_USDT_PCS, CAKE_WBNB_PCS, USDT, WBNB, WBNB_USDT_V2};

/// Адрес исполнителя в тесте (вызов собирается без сети — адрес только кладётся в ExecCall).
const EXECUTOR: Address = address!("0x00000000000000000000000000000000000000ee");
const NOW: u64 = 1_000_000;

fn e18(n: u64) -> U256 {
    U256::from(n) * U256::from(1_000_000_000_000_000_000u128)
}

fn entry(q: &arb::venues::bsc_dex::quote::Quoting, addr: Address) -> Entry {
    q.reg.get(&PoolRef::Contract(addr)).expect("пул в реестре")
}

fn step(
    e: Entry,
    (token_in, token_out): (Address, Address),
    amount_in: U256,
    min_out: U256,
) -> Step {
    Step {
        entry: e,
        order: Order {
            token_in,
            token_out,
            amount_in,
            min_out,
        },
        from_wallet: true,
        to_wallet: true,
    }
}

#[tokio::test]
async fn split_and_two_hop_legs_become_executor_steps() {
    let p = route_common::parts().await;
    p.ctx
        .settings
        .set(
            "dex.deadline_s",
            Scope::Venue("bsc_dex".into()),
            Some(json!(60)),
            "тест",
        )
        .await
        .unwrap();
    let q = p.quoting();
    // Доля 1: 600 USDT → CAKE в PancakeSwap V3; доля 2: 400 USDT → WBNB (PancakeSwap V2) → CAKE (PancakeSwap V3).
    let leg1 = vec![step(
        entry(&q, CAKE_USDT_PCS),
        (USDT, CAKE),
        e18(600),
        e18(290),
    )];
    let leg2 = vec![
        step(entry(&q, WBNB_USDT_V2), (USDT, WBNB), e18(400), e18(1)),
        step(entry(&q, CAKE_WBNB_PCS), (WBNB, CAKE), U256::MAX, e18(190)),
    ];
    let c = call::build(&q.ctx, &[leg1, leg2], EXECUTOR, NOW).expect("вызов собран");
    assert_eq!(
        (c.token_in, c.token_out, c.executor),
        (USDT, CAKE, EXECUTOR)
    );
    assert_eq!(c.amount_in, e18(1000));
    assert_eq!(
        c.min_out,
        e18(480),
        "минимум сделки — сумма минимумов долей"
    );
    assert_eq!(c.deadline, U256::from(NOW + 60));
    let view: Vec<(u8, Address, Address, Address, u16)> = c
        .steps
        .iter()
        .map(|s| (s.kind, s.pool, s.tokenIn, s.tokenOut, s.shareBps))
        .collect();
    assert_eq!(
        view,
        vec![
            (1, CAKE_USDT_PCS, USDT, CAKE, 6000),
            (0, WBNB_USDT_V2, USDT, WBNB, 10_000),
            (1, CAKE_WBNB_PCS, WBNB, CAKE, 10_000),
        ]
    );
    // Комиссия пары V2 — своя у пары (Pool.fee_pips записи = 2500, 0,25 %).
    assert_eq!(u32::abi_decode(&c.steps[1].data).unwrap(), 2500);
    let back = ArbExecutor::executeCall::abi_decode(&c.data()).unwrap();
    assert_eq!(
        (back.amountIn, back.minOut, back.steps.len()),
        (c.amount_in, c.min_out, 3)
    );
}

#[tokio::test]
async fn three_legs_shares_follow_remaining_balance() {
    let p = route_common::parts().await;
    p.ctx
        .settings
        .set(
            "dex.deadline_s",
            Scope::Venue("bsc_dex".into()),
            Some(json!(60)),
            "тест",
        )
        .await
        .unwrap();
    let q = p.quoting();
    let one = |n| vec![step(entry(&q, CAKE_USDT_PCS), (USDT, CAKE), e18(n), e18(1))];
    let c = call::build(&q.ctx, &[one(500), one(250), one(250)], EXECUTOR, NOW).unwrap();
    // 500 из 1000 = 5000 bps; 250 из оставшихся 500 = 5000 bps; последняя — весь остаток.
    let bps: Vec<u16> = c.steps.iter().map(|s| s.shareBps).collect();
    assert_eq!(bps, [5000, 5000, 10_000]);
    // Доли с разным выходом в один вызов не сводятся.
    let other = vec![step(entry(&q, WBNB_USDT_V2), (USDT, WBNB), e18(1), e18(1))];
    assert!(call::build(&q.ctx, &[one(1), other], EXECUTOR, NOW).is_err());
}

#[test]
fn elastic_and_singleton_pools_do_not_go_via_executor() {
    use arb::venues::bsc_dex::exec::executor::entry;
    use arb::venues::bsc_dex::pools::families;
    let pool = PoolRef::Contract(CAKE_USDT_PCS);
    // KyberSwap Elastic: вид ELASTIC в контракте есть, но семейство неисполнимо (решение 121 B2) — шага нет.
    let elastic = families::make("kyberswap-elastic-bsc", "kyber_elastic").unwrap();
    assert!(entry::supported(elastic, &pool).is_err());
    let v3 = families::make("pancakeswap-v3-bsc", "pancakeswap_v3").unwrap();
    assert!(entry::supported(v3, &pool).is_ok());
    // PancakeSwap Infinity CL — только Universal Router.
    let cl = families::make("pancakeswap-infinity-cl-bsc", "pancakeswap_infinity_cl").unwrap();
    assert!(entry::supported(cl, &pool).is_err());
}

// Ревью 122, пункт 2 (свежий минимум верификации не опускает минимум по худшей цене, строже — ставится) — теперь в
// tests/bsc_dex_leg_min.rs вместе с решением 228 (минимум доли считается по всей сделке, exec/leg_min.rs).

/// Ревью стейблсвапов: цикл (вход = выход) из двух долей не собирается — контракт исполняет цикл одной долей («loop»).
#[tokio::test]
async fn loop_with_two_shares_is_refused() {
    let p = route_common::parts().await;
    let q = p.quoting();
    let leg = || {
        vec![
            step(entry(&q, CAKE_USDT_PCS), (USDT, CAKE), e18(10), e18(1)),
            step(entry(&q, CAKE_USDT_PCS), (CAKE, USDT), e18(1), e18(10)),
        ]
    };
    let why = call::build(&q.ctx, &[leg(), leg()], EXECUTOR, NOW).expect_err("цикл из двух долей");
    assert!(why.message.contains("цикл"), "{}", why.message);
}
