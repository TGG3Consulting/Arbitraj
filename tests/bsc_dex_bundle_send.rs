//! Файл: tests/bsc_dex_bundle_send.rs
//! Что делает: отправка бандла (exec/bundle_send.rs, решение 84, фаза 80.8b) и хардинг сборки (exec/bundle.rs, ревью
//! #79) на пулах реальной записи CAKE (tests/route_common), без реальной сети. Offline покрыто: gas_from (лимиты газа
//! из итогов симуляции eth_simulateV1 — gasUsed × надбавка; откат шага → NoRoute); sign_txs (подпись всех транзакций
//! бандла по очереди тестовым ключом — число, различимость, проверки входа); хардинг L2 (нулевой минимум
//! промежуточного шага → отказ) и L3 (разный входной токен ног группы → отказ). Сетью НЕ покрыто (отмечено):
//! реальная симуляция на узле, dispatch_slice в каналы, confirm попадания в блок, оркестрация send() — требуют
//! узла/релея; держатся на типах и переиспользовании проверенных send/*.
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
use arb::venues::bsc_dex::exec::bundle_send::{gas_from, sign_txs};
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

/// Бандл из двух долей на разных роутерах (Pancake + Uniswap), покупка CAKE.
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

#[tokio::test]
async fn gas_from_applies_margin_per_tx() {
    let p = route_common::parts().await;
    let res = [ok_result(100_000), ok_result(250_000)];
    // Надбавка 10% → 110000 и 275000.
    let g = gas_from(&p.ctx, &res, Decimal::from(10u32)).expect("лимиты газа");
    assert_eq!(g, vec![110_000, 275_000]);
}

#[tokio::test]
async fn gas_from_rejects_reverted_step() {
    let p = route_common::parts().await;
    let reverted = CallResult {
        status: U64::from(0u8),
        return_data: Bytes::new(),
        gas_used: U64::from(21_000u64),
        error: Some(CallError {
            code: -32000,
            message: "execution reverted".into(),
        }),
        logs: Vec::new(),
    };
    let err = gas_from(
        &p.ctx,
        &[ok_result(100_000), reverted],
        Decimal::from(10u32),
    )
    .expect_err("откат шага бандла — ошибка");
    assert_eq!(err.code, ErrorCode::NoRoute);
    assert!(err.message.contains("шаг 1"), "{}", err.message);
}

#[tokio::test]
async fn sign_txs_signs_every_bundle_tx_in_order() {
    let p = route_common::parts().await;
    prepare(&p, true).await;
    let b = two_router_bundle(&p).await;
    let key: PrivateKeySigner = KEY.parse().unwrap();
    let price = U256::from(1_000_000_000u64);
    let limits: Vec<u64> = b.txs.iter().map(|_| 300_000u64).collect();
    let bases: HashMap<Address, U256> = [(CAKE, e18(5))].into_iter().collect();
    let signed = sign_txs(&p.ctx, &key, &b, price, &limits, &bases).expect("подпись бандла");
    assert_eq!(signed.len(), b.txs.len(), "подписана каждая транзакция");
    // Хеши различны, байты непусты.
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
    let bases: HashMap<Address, U256> = [(CAKE, e18(5))].into_iter().collect();
    // Лимитов меньше, чем транзакций — отказ.
    let err = sign_txs(&p.ctx, &key, &b, price, &[300_000], &bases).expect_err("мало лимитов");
    assert_eq!(err.code, ErrorCode::Internal);
    // Нет базы выходного токена для BALANCE_CHECK свопа — отказ.
    let limits: Vec<u64> = b.txs.iter().map(|_| 300_000u64).collect();
    let err = sign_txs(&p.ctx, &key, &b, price, &limits, &HashMap::new()).expect_err("нет базы");
    assert_eq!(err.code, ErrorCode::Internal);
}

#[tokio::test]
async fn hardening_rejects_zero_intermediate_min() {
    // L2: нулевой минимум промежуточного шага двуххоповой ноги — отказ (как exec/plan.rs::build).
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
    // L3: две ноги одного роутера с разным входным токеном — отказ (разрешения и вход бандла не свести).
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
    // Вторая нога того же Pancake-роутера, но вход WBNB (искусственно) — должна быть отвергнута.
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
