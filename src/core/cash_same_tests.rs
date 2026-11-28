//! Файл: core/cash_same_tests.rs
//! Что проверяет: равные имена USDT у DEX с dex.settlement_contract и биржи (найдено арбитром 28.11.2026) — не денежный
//! круг (settlement_market::mixed = нет), а явная причина «адрес USDT сети не подтверждён», пока адрес USDT сети по
//! биржам не подтверждён или не совпал; совпал — прямой расчёт без причины. Разные имена (USDC на DEX Base ↔ USDT
//! биржи) — денежный круг, как раньше. Адреса: USDT Ethereum и WETH — из записанных пулов
//! tests/data/base_ethereum_v2_probes.json; USDC Base — из записанного справочника tests/data/bingx_2026-11-21.
//! Кто вызывает: cargo test.
//! От чего зависит: cash_same.rs, settlement_market.rs (mixed), test_core.rs (ядро на временной SQLite, реестр пуст).

use alloy_primitives::{Address, address};
use chrono::Utc;
use serde_json::json;

use super::check;
use crate::core::engine::wr;
use crate::core::settlement_market::mixed;
use crate::core::types::{Asset, Direction, VenueId};
use crate::settings::value::Scope;

const USDT_ETH: Address = address!("0xdac17f958d2ee523a2206206994597c13d831ec7");
const WETH: Address = address!("0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2");
const USDC_BASE: Address = address!("0x833589fcd6edb6e08f4c7c32d4f71b54bda02913");

fn dir(dex: &str) -> Direction {
    Direction {
        asset: Asset {
            chain: None,
            ticker: "X".into(),
            contract: None,
        },
        buy_on: VenueId("gate".into()),
        sell_on: VenueId(dex.into()),
    }
}

#[tokio::test]
async fn same_usdt_labels_are_direct_with_explicit_reason() {
    let path = std::env::temp_dir().join(format!(
        "arb-cash-same-{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap()
    ));
    std::fs::create_dir_all(&path).unwrap();
    let core = crate::core::test_core::open(&path).await;
    let s = core.inner.deps.settings.clone();
    let venue = |v: &str| Scope::Venue(v.into());
    for (name, scope, value) in [
        ("venue.quote_asset", venue("gate"), json!("USDT")),
        ("venue.quote_asset", venue("eth_dex"), json!("USDT")),
        ("dex.chain", venue("eth_dex"), json!("ethereum")),
        (
            "dex.settlement_contract",
            venue("eth_dex"),
            json!(USDT_ETH.to_string()),
        ),
        ("venue.quote_asset", venue("base_dex"), json!("USDC")),
        ("dex.chain", venue("base_dex"), json!("base")),
        (
            "dex.settlement_contract",
            venue("base_dex"),
            json!(USDC_BASE.to_string()),
        ),
    ] {
        s.set(name, scope, Some(value), "тест").await.unwrap();
    }
    let eth = dir("eth_dex");
    // Адрес USDT сети биржами не подтверждён: не денежный круг, а явная причина (раньше — вечное «совпадают»).
    assert!(!mixed(&core, &eth));
    let e = check(&core, &eth).unwrap_err();
    assert!(e.starts_with("адрес USDT сети не подтверждён"), "{e}");
    // Биржи сообщили другой адрес — причина с обоими адресами.
    *wr(&core.inner.usdt) = Ok(WETH);
    assert!(!mixed(&core, &eth));
    let e = check(&core, &eth).unwrap_err();
    assert!(e.contains("≠ USDT сети по биржам"), "{e}");
    // Совпал — прямой расчёт без причины (как до изменения).
    *wr(&core.inner.usdt) = Ok(USDT_ETH);
    assert!(!mixed(&core, &eth));
    check(&core, &eth).unwrap();
    // Разные имена (USDC на DEX ↔ USDT биржи) — денежный круг, проверка равных имён его не касается.
    let base = dir("base_dex");
    assert!(mixed(&core, &base));
    check(&core, &base).unwrap();
}
