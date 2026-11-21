//! Файл: tests/bsc_dex_bundle_native.rs
//! Что делает: решение 122, часть R3 — нативная монета BNB в бандле свопов (exec/bundle_native.rs, exec/bundle.rs) на
//! пулах РЕАЛЬНОЙ записи (tests/route_common, routes_2026-10-29): монета сделки — WBNB. Покупка: ноги отдают WBNB
//! роутеру, последняя команда execute — UNWRAP_WETH(кошелёк, сумма минимумов ног роутера), value 0, разрешение на USDT
//! есть. Продажа: value транзакции = сумма входов ног роутера, первая команда — WRAP_ETH(роутер, сумма), первый шаг
//! каждой ноги платит роутер (разрешений нет). Ноги одной группы, обёрнутые по-разному, — отказ. Исполнимость графа:
//! BNB через ногу форка — сигнал с причиной. Без сети (all_approvals = true).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::exec::{bundle, bundle_native, chain, plan, ur}, tests/route_common.

mod route_common;

use alloy::sol_types::{SolCall, SolValue};
use alloy_primitives::{Address, Bytes, U256, address};
use serde_json::json;

use arb::core::types::{PoolRef, Side};
use arb::gateway::errors::ErrorCode;
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::exec::bundle::{self, Bundle, BundleTx};
use arb::venues::bsc_dex::exec::bundle_native;
use arb::venues::bsc_dex::exec::chain::Step;
use arb::venues::bsc_dex::exec::plan::Order;
use arb::venues::bsc_dex::exec::ur::{self, IUniversalRouter};
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::quote::Quoting;

use route_common::{CAKE, CAKE_USDT_PCS, CAKE_WBNB_PCS, USDT, WBNB, WBNB_USDT_V2};

const WALLET: Address = address!("0x00000000000000000000000000000000000000aa");

fn e18(n: u64) -> U256 {
    U256::from(n) * U256::from(1_000_000_000_000_000_000u128)
}

async fn prepare(p: &Parts) {
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
}

fn step(q: &Quoting, pool: Address, (tin, tout): (Address, Address), amount: U256) -> Step {
    Step {
        entry: q.reg.get(&PoolRef::Contract(pool)).expect("пул записи"),
        order: Order {
            token_in: tin,
            token_out: tout,
            amount_in: amount,
            min_out: e18(1),
        },
        from_wallet: true,
        to_wallet: true,
    }
}

/// Две ноги одного роутера с WBNB на стороне монеты: прямая WBNB — USDT (V2) и через CAKE (два V3 PancakeSwap).
fn legs(q: &Quoting, side: Side) -> Vec<Vec<Step>> {
    match side {
        Side::Buy => {
            let mut b = vec![
                step(q, CAKE_USDT_PCS, (USDT, CAKE), e18(300)),
                step(q, CAKE_WBNB_PCS, (CAKE, WBNB), ur::CONTRACT_BALANCE),
            ];
            (b[0].to_wallet, b[1].from_wallet) = (false, false);
            vec![vec![step(q, WBNB_USDT_V2, (USDT, WBNB), e18(600))], b]
        }
        Side::Sell => {
            let mut b = vec![
                step(q, CAKE_WBNB_PCS, (WBNB, CAKE), e18(2)),
                step(q, CAKE_USDT_PCS, (CAKE, USDT), ur::CONTRACT_BALANCE),
            ];
            (b[0].to_wallet, b[1].from_wallet) = (false, false);
            vec![vec![step(q, WBNB_USDT_V2, (WBNB, USDT), e18(1))], b]
        }
    }
}

/// Команды execute единственного свопа бандла и сам план. Ошибок нет (тест падает).
fn swap(b: &Bundle) -> (Vec<u8>, Vec<Bytes>, U256) {
    let plan = b
        .txs
        .iter()
        .find_map(|t| match t {
            BundleTx::Swap { plan, .. } => Some(plan.clone()),
            _ => None,
        })
        .expect("своп в бандле");
    let call = IUniversalRouter::executeCall::abi_decode(&plan.data(U256::ZERO)).unwrap();
    (call.commands.to_vec(), call.inputs, plan.value())
}

#[tokio::test]
async fn buy_unwraps_on_each_router() {
    let p = route_common::parts().await;
    prepare(&p).await;
    let q = p.quoting();
    let mut l = legs(&q, Side::Buy);
    bundle_native::adapt(&q.ctx, Side::Buy, &mut l).expect("V2/V3 на стороне BNB");
    assert!(l.iter().all(|leg| !leg.last().unwrap().to_wallet));
    let b = bundle::build(&q.ctx, &l, WBNB, WALLET, 1, 1_000_000, true)
        .await
        .expect("бандл");
    let (cmds, inputs, value) = swap(&b);
    assert_eq!(value, U256::ZERO);
    assert_eq!(*cmds.last().unwrap(), ur::UNWRAP_WETH);
    let (to, min) = <(Address, U256)>::abi_decode_params(inputs.last().unwrap()).unwrap();
    assert_eq!(
        (to, min),
        (WALLET, e18(2)),
        "весь WBNB роутера — кошельку, минимум — сумма минимумов ног"
    );
    // Разрешение Permit2 на USDT есть (платит кошелёк).
    assert!(b.txs.iter().any(|t| matches!(t, BundleTx::Approve { .. })));
}

#[tokio::test]
async fn sell_wraps_value_on_each_router() {
    let p = route_common::parts().await;
    prepare(&p).await;
    let q = p.quoting();
    let mut l = legs(&q, Side::Sell);
    bundle_native::adapt(&q.ctx, Side::Sell, &mut l).expect("V2/V3 на стороне BNB");
    let b = bundle::build(&q.ctx, &l, USDT, WALLET, 1, 1_000_000, true)
        .await
        .expect("бандл");
    let (cmds, inputs, value) = swap(&b);
    assert_eq!(value, e18(3), "value = сумма входов ног роутера");
    assert_eq!(cmds[0], ur::WRAP_ETH);
    let (to, amount) = <(Address, U256)>::abi_decode_params(&inputs[0]).unwrap();
    assert_eq!((to, amount), (ur::ADDRESS_THIS, e18(3)));
    assert_eq!(*cmds.last().unwrap(), ur::BALANCE_CHECK_ERC20);
    // Первый шаг каждой ноги платит роутер — разрешений кошелька нет.
    assert!(!b.txs.iter().any(|t| matches!(t, BundleTx::Approve { .. })));
    assert_eq!(bundle_native::routers(&b).len(), 1);
}

#[tokio::test]
async fn mixed_wrap_in_group_refused() {
    let p = route_common::parts().await;
    prepare(&p).await;
    let q = p.quoting();
    let mut l = legs(&q, Side::Sell);
    // Обёрнута только первая нога — вторая платит кошелёк: план одной группы не свести.
    l[0][0].from_wallet = false;
    let e = bundle::build(&q.ctx, &l, USDT, WALLET, 1, 1_000_000, true)
        .await
        .expect_err("ноги обёрнуты по-разному");
    assert_eq!(e.code, ErrorCode::Internal);
}
