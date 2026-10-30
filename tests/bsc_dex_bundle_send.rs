//! Файл: tests/bsc_dex_bundle_send.rs
//! Что делает: отправка бандла (exec/bundle_send.rs, решение 84, фаза 80.8b/8c) и хардинг сборки (exec/bundle.rs,
//! ревью #79) на пулах реальной записи CAKE (tests/route_common), без реальной сети. Offline покрыто: collect (лимиты
//! газа из итогов симуляции eth_simulateV1 — gasUsed × надбавка — и КУМУЛЯТИВНАЯ база BALANCE_CHECK каждого свопа из
//! balanceOf по состоянию после предыдущих групп, ревью #80 MEDIUM); откат шага → NoRoute; sign_txs (подпись всех
//! транзакций бандла по очереди тестовым ключом — число, различимость, база по свопу); хардинг L2 (нулевой минимум
//! промежуточного шага → отказ) и L3 (разный входной токен ног группы → отказ). Сетью НЕ покрыто (отмечено):
//! реальная симуляция на узле, dispatch_slice в каналы, confirm попадания в блок, оркестрация send().
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::exec::{bundle, bundle_send, chain, plan}, check::simulate, tests/route_common.

mod route_common;

use std::collections::HashMap;

use alloy::signers::local::PrivateKeySigner;
use alloy_primitives::{Address, Bytes, U64, U256, address};
use rust_decimal::Decimal;
use serde_json::json;

use arb::core::types::PoolRef;
use arb::gateway::errors::ErrorCode;
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::check::simulate::{CallError, CallResult};
use arb::venues::bsc_dex::exec::bundle::{self, Bundle};
use arb::venues::bsc_dex::exec::bundle_send::{build_sim, collect, sign_txs};
use arb::venues::bsc_dex::exec::chain::Step;
use arb::venues::bsc_dex::exec::plan::Order;
use arb::venues::bsc_dex::pools::registry::Entry;

use route_common::{CAKE, CAKE_USDT_PCS, CAKE_USDT_UNI, CAKE_WBNB_PCS, USDT, WBNB, WBNB_USDT_V2};

const WALLET: Address = address!("0x00000000000000000000000000000000000000aa");
const FIRST_NONCE: u64 = 7;
const NOW: u64 = 1_000_000;
/// Тестовый ключ из официального примера EIP-155 (wallet/signer.rs).
const KEY: &str = "0x4646464646464646464646464646464646464646464646464646464646464646";

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

/// Бандл из двух долей покупки CAKE на разных роутерах (Pancake + Uniswap) — обе выдают CAKE в кошелёк.
async fn two_router_bundle(p: &arb::venues::bsc_dex::parts::Parts) -> Bundle {
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
    bundle::build(
        &q.ctx,
        &[leg_pcs, leg_uni],
        CAKE,
        WALLET,
        FIRST_NONCE,
        NOW,
        true,
    )
    .await
    .expect("бандл собран")
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

/// Итог balanceOf: 32 байта big-endian баланса.
fn bal_result(v: U256) -> CallResult {
    CallResult {
        status: U64::from(1u8),
        return_data: Bytes::from(v.to_be_bytes::<32>().to_vec()),
        gas_used: U64::from(2_300u64),
        error: None,
        logs: Vec::new(),
    }
}

#[tokio::test]
async fn collect_gas_and_cumulative_base_per_group() {
    // Ревью #80 MEDIUM: сплит одной монеты через два роутера — база BALANCE_CHECK второй группы учитывает выход первой
    // (кумулятивно), а не берётся тем же до-бандл балансом. Итоги симуляции: перед свопом 1 кошелёк имел `pre`, перед
    // свопом 2 (после группы 1) — `pre + out1`.
    let p = route_common::parts().await;
    prepare(&p, true).await;
    let b = two_router_bundle(&p).await;
    let plan = build_sim(&b, WALLET);
    assert_eq!(plan.bal.len(), 2, "balanceOf перед каждым свопом");
    let pre = e18(5);
    let out1 = e18(490);
    // Крафт итогов: все вызовы успешны, лимит газа 100000; два balanceOf дают pre и pre+out1.
    let mut res: Vec<CallResult> = plan.calls.iter().map(|_| ok_result(100_000)).collect();
    res[plan.bal[0].1] = bal_result(pre);
    res[plan.bal[1].1] = bal_result(pre + out1);
    let (limits, bases) =
        collect(&p.ctx, &res, &plan, Decimal::from(10u32)).expect("сбор газа и баз");
    assert_eq!(limits.len(), b.txs.len());
    assert!(limits.iter().all(|&g| g == 110_000), "gasUsed 100000 × 1.1");
    let swap1 = plan.bal[0].0;
    let swap2 = plan.bal[1].0;
    assert_eq!(bases[&swap1], pre, "база первой группы — до-бандл баланс");
    assert_eq!(
        bases[&swap2],
        pre + out1,
        "база второй группы — после выхода первой"
    );
    assert!(
        bases[&swap2] > bases[&swap1],
        "база второй группы учитывает выход первой (кумулятивно)"
    );
}

#[tokio::test]
async fn collect_rejects_reverted_step() {
    let p = route_common::parts().await;
    prepare(&p, true).await;
    let b = two_router_bundle(&p).await;
    let plan = build_sim(&b, WALLET);
    let mut res: Vec<CallResult> = plan.calls.iter().map(|_| ok_result(100_000)).collect();
    res[plan.bal[0].1] = bal_result(e18(5));
    res[plan.bal[1].1] = bal_result(e18(5));
    // Второй своп откатился в симуляции.
    let swap2_call = plan.main[plan.bal[1].0];
    res[swap2_call] = CallResult {
        status: U64::from(0u8),
        return_data: Bytes::new(),
        gas_used: U64::from(21_000u64),
        error: Some(CallError {
            code: -32000,
            message: "execution reverted".into(),
        }),
        logs: Vec::new(),
    };
    let err =
        collect(&p.ctx, &res, &plan, Decimal::from(10u32)).expect_err("откат шага бандла — ошибка");
    assert_eq!(err.code, ErrorCode::NoRoute);
}

#[tokio::test]
async fn sign_txs_signs_every_bundle_tx_in_order() {
    let p = route_common::parts().await;
    prepare(&p, true).await;
    let b = two_router_bundle(&p).await;
    let key: PrivateKeySigner = KEY.parse().unwrap();
    let price = U256::from(1_000_000_000u64);
    let limits: Vec<u64> = b.txs.iter().map(|_| 300_000u64).collect();
    // Базы BALANCE_CHECK по индексам транзакций-свопов.
    let bases: HashMap<usize, U256> = b
        .txs
        .iter()
        .enumerate()
        .filter(|(_, t)| t.is_swap())
        .map(|(i, _)| (i, e18(5)))
        .collect();
    let signed = sign_txs(&p.ctx, &key, &b, price, &limits, &bases).expect("подпись бандла");
    assert_eq!(signed.len(), b.txs.len(), "подписана каждая транзакция");
    let mut hashes: Vec<_> = signed.iter().map(|s| s.hash).collect();
    hashes.sort();
    hashes.dedup();
    assert_eq!(hashes.len(), signed.len(), "хеши транзакций различны");
    assert!(signed.iter().all(|s| !s.raw.is_empty()));
}

#[tokio::test]
async fn sign_txs_needs_limit_per_tx_and_base() {
    let p = route_common::parts().await;
    prepare(&p, true).await;
    let b = two_router_bundle(&p).await;
    let key: PrivateKeySigner = KEY.parse().unwrap();
    let price = U256::from(1_000_000_000u64);
    // Лимитов меньше, чем транзакций — отказ.
    let err =
        sign_txs(&p.ctx, &key, &b, price, &[300_000], &HashMap::new()).expect_err("мало лимитов");
    assert_eq!(err.code, ErrorCode::Internal);
    // Нет базы свопа для BALANCE_CHECK — отказ.
    let limits: Vec<u64> = b.txs.iter().map(|_| 300_000u64).collect();
    let err = sign_txs(&p.ctx, &key, &b, price, &limits, &HashMap::new()).expect_err("нет базы");
    assert_eq!(err.code, ErrorCode::Internal);
}

#[tokio::test]
async fn hardening_rejects_zero_intermediate_min() {
    // L2 (ревью #79): нулевой минимум промежуточного шага двуххоповой ноги — отказ (как exec/plan.rs::build).
    let p = route_common::parts().await;
    prepare(&p, false).await;
    let q = p.quoting();
    let leg = vec![
        step(
            entry(&q, WBNB_USDT_V2),
            USDT,
            WBNB,
            e18(1000),
            U256::ZERO,
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
        .expect_err("нулевой минимум промежуточного шага — отказ");
    assert_eq!(err.code, ErrorCode::QuoteMismatch);
}

#[tokio::test]
async fn hardening_rejects_mixed_input_token_in_group() {
    // L3 (ревью #79): две ноги одного роутера с разным входным токеном — отказ.
    let p = route_common::parts().await;
    prepare(&p, false).await;
    let q = p.quoting();
    let leg_usdt = vec![step(
        entry(&q, CAKE_USDT_PCS),
        USDT,
        CAKE,
        e18(1000),
        e18(490),
        true,
        true,
    )];
    let leg_wbnb = vec![step(
        entry(&q, CAKE_WBNB_PCS),
        WBNB,
        CAKE,
        e18(5),
        e18(480),
        true,
        true,
    )];
    let err = bundle::build(
        &q.ctx,
        &[leg_usdt, leg_wbnb],
        CAKE,
        WALLET,
        FIRST_NONCE,
        NOW,
        true,
    )
    .await
    .expect_err("разный входной токен группы — отказ");
    assert_eq!(err.code, ErrorCode::Internal);
}
