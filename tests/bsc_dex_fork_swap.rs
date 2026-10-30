//! Файл: tests/bsc_dex_fork_swap.rs
//! Что делает: сборка/симуляция/подпись свопа через СОБСТВЕННЫЙ роутер форка (exec/fork_swap.rs, exec/bundle.rs
//! BundleTx::ForkSwap, решения 59, 82, фаза 80.9b) на РЕАЛЬНЫХ резервах V2-пула записи routes_2026-10-29 (меняется
//! только метка семейства — v2_fork — и токены пары; к сети тест не ходит). Покрыто offline: нога форка → ERC-20
//! approve роутеру (без Permit2) + ForkSwap с путём/суммой/минимумом; смешанный бандл UR (Pancake) + форк (цель
//! решения 80) собирается, симулируется и подписывается; у форк-свопа НЕТ базы BALANCE_CHECK (защита — amountOutMin
//! роутера, решение 40); нулевой минимум и смешанная (форк+не-форк) нога — отказ.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::exec::{bundle, bundle_sim, fork_swap, chain, plan}, pools::families,
//! check::simulate, tests/route_common (реальная запись и окружение).

mod route_common;

use std::collections::HashMap;

use alloy::signers::local::PrivateKeySigner;
use alloy_primitives::{Address, Bytes, U64, U256, address};
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::json;

use arb::core::types::{Pool, PoolRef, PoolState, VenueId};
use arb::gateway::errors::ErrorCode;
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::check::simulate::CallResult;
use arb::venues::bsc_dex::exec::bundle::{self, BundleTx};
use arb::venues::bsc_dex::exec::bundle_sim::{build_sim, collect, sign_txs};
use arb::venues::bsc_dex::exec::chain::Step;
use arb::venues::bsc_dex::exec::plan::Order;
use arb::venues::bsc_dex::pools::families;
use arb::venues::bsc_dex::pools::registry::Entry;
use arb::venues::bsc_dex::quote::Quoting;

use route_common::{CAKE, CAKE_USDT_PCS, CAKE_WBNB_PCS, USDT, WBNB, WBNB_USDT_V2};

const WALLET: Address = address!("0x00000000000000000000000000000000000000aa");
const FORK_ROUTER: Address = address!("0x00000000000000000000000000000000000000b1");
const FORK_POOL: Address = address!("0x00000000000000000000000000000000000000b2");
const FIRST_NONCE: u64 = 7;
const NOW: u64 = 1_000_000;
/// Тестовый ключ из официального примера EIP-155 (wallet/signer.rs).
const KEY: &str = "0x4646464646464646464646464646464646464646464646464646464646464646";

fn e18(n: u64) -> U256 {
    U256::from(n) * U256::from(1_000_000_000_000_000_000u128)
}

fn entry(q: &Quoting, addr: Address) -> Entry {
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

/// Пул CAKE-USDT семейства v2_fork (Biswap) с РЕАЛЬНЫМИ резервами V2-пула записи (WBNB-USDT v2) — меняется только метка
/// семейства и токены пары (магнитуды реальные). fork=true → bundle::build ведёт его ногу через роутер форка.
fn fork_entry(q: &Quoting) -> Entry {
    let real = entry(q, WBNB_USDT_V2);
    let (r0, r1) = match real.pool.state {
        PoolState::V2 { reserve0, reserve1 } => (reserve0, reserve1),
        _ => panic!("WBNB-USDT v2 — не V2-состояние"),
    };
    let f = families::make("biswap", "v2_fork").unwrap();
    assert!(f.fork && f.graphable, "v2_fork — форк и в графе");
    let (t0, t1) = if CAKE < USDT {
        (CAKE, USDT)
    } else {
        (USDT, CAKE)
    };
    Entry {
        family: f,
        pool: Pool {
            venue: VenueId("bsc_dex".into()),
            family: f.id.into(),
            kind: f.kind,
            reference: PoolRef::Contract(FORK_POOL),
            token0: t0,
            token1: t1,
            fee_pips: Some(1000),
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
        ticker: "CAKE".into(),
        coin: CAKE,
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

/// Настройки для сборки бандла: дедлайн/approve + роутер форка Biswap (его dex.router, без Universal Router/Permit2).
async fn prepare(p: &arb::venues::bsc_dex::parts::Parts) {
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
    p.ctx
        .settings
        .set(
            "dex.router",
            Scope::Dex("biswap".into()),
            Some(json!(format!("{FORK_ROUTER}"))),
            "тест",
        )
        .await
        .unwrap();
}

fn ok_result(gas: u64) -> CallResult {
    CallResult {
        status: U64::from(1u8),
        return_data: Bytes::new(),
        gas_used: U64::from(gas),
        error: None,
        logs: Vec::new(),
    }
}

fn bal_result(v: U256) -> CallResult {
    CallResult {
        status: U64::from(1u8),
        return_data: Bytes::from(v.to_be_bytes::<32>().to_vec()),
        gas_used: U64::from(2_300u64),
        error: None,
        logs: Vec::new(),
    }
}

/// Нога форка собирается в ERC-20 approve роутеру (без Permit2) + BundleTx::ForkSwap с путём/суммой/минимумом.
#[tokio::test]
async fn fork_leg_builds_approve_and_forkswap() {
    let p = route_common::parts().await;
    prepare(&p).await;
    let q = p.quoting();
    let leg = vec![step(
        fork_entry(&q),
        USDT,
        CAKE,
        e18(1000),
        e18(490),
        true,
        true,
    )];
    let b = bundle::build(&q.ctx, &[leg], CAKE, WALLET, FIRST_NONCE, NOW, true)
        .await
        .expect("бандл форка собран");
    assert_eq!(b.swaps, 1, "один своп форка");
    assert_eq!(b.txs.len(), 2, "approve + форк-своп");
    match &b.txs[0] {
        BundleTx::Approve { to, nonce, .. } => {
            assert_eq!(*to, USDT, "approve входного токена (USDT)");
            assert_eq!(*nonce, FIRST_NONCE);
        }
        _ => panic!("первая транзакция — ERC-20 approve"),
    }
    match &b.txs[1] {
        BundleTx::ForkSwap {
            router,
            path,
            amount_in,
            min_out,
            to,
            nonce,
            ..
        } => {
            assert_eq!(*router, FORK_ROUTER);
            assert_eq!(path.as_slice(), [USDT, CAKE]);
            assert_eq!(*amount_in, e18(1000));
            assert_eq!(*min_out, e18(490));
            assert_eq!(*to, WALLET);
            assert_eq!(*nonce, FIRST_NONCE + 1);
        }
        _ => panic!("вторая транзакция — форк-своп"),
    }
}

/// Смешанный бандл: нога UR (Pancake) + нога форка (Biswap) — обе в одном атомарном бандле (цель решения 80);
/// симулируется и подписывается; balanceOf-база только у UR-свопа (у форка защита в amountOutMin).
#[tokio::test]
async fn mixed_ur_and_fork_bundle() {
    let p = route_common::parts().await;
    prepare(&p).await;
    let q = p.quoting();
    let leg_ur = vec![step(
        entry(&q, CAKE_USDT_PCS),
        USDT,
        CAKE,
        e18(1000),
        e18(495),
        true,
        true,
    )];
    let leg_fork = vec![step(
        fork_entry(&q),
        USDT,
        CAKE,
        e18(1000),
        e18(490),
        true,
        true,
    )];
    let b = bundle::build(
        &q.ctx,
        &[leg_ur, leg_fork],
        CAKE,
        WALLET,
        FIRST_NONCE,
        NOW,
        true,
    )
    .await
    .expect("смешанный бандл собран");
    assert_eq!(b.swaps, 2, "UR-своп + форк-своп");
    assert!(
        b.txs.iter().any(|t| matches!(t, BundleTx::Swap { .. })),
        "есть UR-своп"
    );
    assert!(
        b.txs.iter().any(|t| matches!(t, BundleTx::ForkSwap { .. })),
        "есть форк-своп"
    );
    let plan = build_sim(&b, WALLET);
    assert_eq!(
        plan.bal.len(),
        1,
        "balanceOf-база только у UR-свопа (форк защищён amountOutMin)"
    );
    let mut res: Vec<CallResult> = plan.calls.iter().map(|_| ok_result(100_000)).collect();
    res[plan.bal[0].1] = bal_result(e18(5));
    let (limits, bases) =
        collect(&p.ctx, &res, &plan, Decimal::from(10u32)).expect("сбор газа и баз");
    assert_eq!(limits.len(), b.txs.len());
    let key: PrivateKeySigner = KEY.parse().unwrap();
    let signed = sign_txs(
        &p.ctx,
        &key,
        &b,
        U256::from(1_000_000_000u64),
        &limits,
        &bases,
    )
    .expect("подпись смешанного бандла");
    assert_eq!(signed.len(), b.txs.len(), "подписана каждая транзакция");
    let mut hashes: Vec<_> = signed.iter().map(|s| s.hash).collect();
    hashes.sort();
    hashes.dedup();
    assert_eq!(hashes.len(), signed.len(), "хеши различны");
}

/// Чисто форк-бандл: симуляция без balanceOf-баз, подпись проходит с пустыми базами (у форк-свопа базы нет).
#[tokio::test]
async fn fork_bundle_simulated_and_signed_without_base() {
    let p = route_common::parts().await;
    prepare(&p).await;
    let q = p.quoting();
    let leg = vec![step(
        fork_entry(&q),
        USDT,
        CAKE,
        e18(1000),
        e18(490),
        true,
        true,
    )];
    let b = bundle::build(&q.ctx, &[leg], CAKE, WALLET, FIRST_NONCE, NOW, true)
        .await
        .expect("бандл форка");
    let plan = build_sim(&b, WALLET);
    assert!(plan.bal.is_empty(), "у форк-свопа нет базы BALANCE_CHECK");
    let res: Vec<CallResult> = plan.calls.iter().map(|_| ok_result(90_000)).collect();
    let (limits, bases) = collect(&p.ctx, &res, &plan, Decimal::from(10u32)).expect("сбор");
    assert!(bases.is_empty(), "баз BALANCE_CHECK нет");
    let key: PrivateKeySigner = KEY.parse().unwrap();
    let signed = sign_txs(
        &p.ctx,
        &key,
        &b,
        U256::from(1_000_000_000u64),
        &limits,
        &HashMap::new(),
    )
    .expect("подпись форк-бандла без баз");
    assert_eq!(signed.len(), b.txs.len());
}

/// Нулевой минимум ноги форка — отказ (обмен без минимума не отправляется, решение 40).
#[tokio::test]
async fn fork_leg_zero_min_rejected() {
    let p = route_common::parts().await;
    prepare(&p).await;
    let q = p.quoting();
    let leg = vec![step(
        fork_entry(&q),
        USDT,
        CAKE,
        e18(1000),
        U256::ZERO,
        true,
        true,
    )];
    let err = bundle::build(&q.ctx, &[leg], CAKE, WALLET, FIRST_NONCE, NOW, true)
        .await
        .expect_err("нулевой минимум форк-ноги — отказ");
    assert_eq!(err.code, ErrorCode::QuoteMismatch);
}

/// Нога, смешивающая форк и не-форк (один вызов роутера её не исполнит) — отказ.
#[tokio::test]
async fn fork_leg_mixed_with_non_fork_rejected() {
    let p = route_common::parts().await;
    prepare(&p).await;
    let q = p.quoting();
    let leg = vec![
        step(fork_entry(&q), USDT, WBNB, e18(1000), e18(1), true, false),
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
        .expect_err("смешанная форк/не-форк нога — отказ");
    assert_eq!(err.code, ErrorCode::NoRoute);
}
