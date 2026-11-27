//! Публикация индексов маршрутов и блокировка реестра при выборе котировки на BSC.
#[path = "registry_path_fixtures.rs"]
mod fixtures;
use super::{addr, entry};
use crate::core::types::{Asset, MotoswapState, PoolRef, PoolState, Side, TokenTaxes};
use crate::venues::bsc_dex::{
    hashflow::recovery_tests,
    pools::{families, registry::{Entry, Registry}, registry_frozen::Want},
    quote::{fluid_hop_pricing, routing::Input},
};
use alloy_primitives::{Address, B256, U256};
use flate2::read::GzDecoder;
use rust_decimal::Decimal;
use serde_json::Value;
use std::{sync::mpsc, time::Duration};

fn ambient(reg: &Registry) -> (Entry, B256) {
    let a: Value = serde_json::from_reader(GzDecoder::new(
        &include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/ambient_ethereum_silent_swap.json.gz")[..],
    )).unwrap();
    let n = 26_492_698;
    let hash: B256 = a["headers"][0]["hash"].as_str().unwrap().parse().unwrap();
    reg.ambient_head(Some((n, hash)));
    let row = &a["states"][0][0];
    (entry(reg, &a, row, n, hash), hash)
}

#[test]
fn recorded_motoswap_uses_its_own_plan_and_block_refresh_on_ethereum() {
    use crate::venues::bsc_dex::{exec::motoswap_plan, pools::per_block, quote::v2_pair_quote};

    let moto = fixtures::moto();
    let pancake = fixtures::pancake();
    assert!(moto.family.variant.motoswap());
    assert!(per_block::needs(moto.family));
    assert_eq!(motoswap_plan::shape(&moto), Some(moto.pool.reference.address()));
    assert!(!v2_pair_quote::on(&moto));

    assert!(!pancake.family.variant.motoswap());
    assert!(!per_block::needs(pancake.family));
    assert_eq!(motoswap_plan::shape(&pancake), None);

    let mut mismatched = moto;
    mismatched.family = pancake.family;
    assert_eq!(motoswap_plan::shape(&mismatched), None);
}

#[test]
fn path_and_head_indexes_follow_replacement_update_retain_and_snapshot() {
    let reg = Registry::default();
    let watch = reg.clone();
    let (ambient, hash) = ambient(&reg);
    let moto = fixtures::moto();
    let r = &moto.pool.reference;
    reg.put(ambient.clone());
    assert!(watch.watches_ambient());
    assert!(watch.multi_market_edges().is_empty());
    reg.put(moto.clone());
    assert!(watch.watches_ambient());
    assert_eq!(watch.multi_market_edges().len(), 1);
    reg.retain(|e| e.pool.reference != ambient.pool.reference);
    assert!(!watch.watches_ambient());
    assert_eq!(watch.multi_market_edges().len(), 1);
    let snap = reg.freeze(&Want {
        coin: moto.coin, usdt: moto.usdt, mids: &[], wbnb: None, paths: &[],
    });
    let frozen = Registry::frozen_from(&snap, None);

    // Исправление классификации одного ключа при прежних ключах пары.
    let mut replacement = moto.clone();
    replacement.pool.state = ambient.pool.state.clone();
    reg.update(r, |e| e.pool.state = replacement.pool.state.clone()).unwrap();
    assert!(watch.watches_ambient());
    assert!(watch.multi_market_edges().is_empty());
    reg.update(r, |e| e.pool.state = moto.pool.state.clone()).unwrap();
    assert!(!watch.watches_ambient());
    assert_eq!(watch.multi_market_edges().len(), 1);
    // Стандартная замена через put, включая возврат последнего пути.
    reg.ambient_head(Some((ambient.pool.block, hash)));
    reg.put(replacement);
    assert!(watch.watches_ambient());
    assert!(watch.multi_market_edges().is_empty());
    reg.put(moto);
    assert!(!watch.watches_ambient());
    assert_eq!(watch.multi_market_edges().len(), 1);
    reg.retain(|_| false);
    assert!(watch.multi_market_edges().is_empty());
    assert!(!watch.has_market_paths.load(std::sync::atomic::Ordering::Acquire));
    assert_eq!(frozen.multi_market_edges().len(), 1);
}

#[tokio::test]
async fn bsc_quote_checks_paths_while_pool_registry_is_write_locked() {
    let dir = recovery_tests::directory("registry-paths", 56);
    let parts = recovery_tests::parts(&dir, 56, Address::ZERO).await;
    let q = parts.quoting();
    let pair = fixtures::pancake();
    assert!(!pair.family.variant.fluid_lite());
    assert!(crate::venues::bsc_dex::pools::fluid_dex_lite::shape(&pair).is_none());
    q.reg.put(pair.clone());
    let input = Input {
        asset: Asset { chain: None, ticker: pair.ticker.clone(), contract: Some(pair.coin) },
        side: Side::Buy,
        qty: Decimal::ONE,
        coin: pair.coin,
        usdt: pair.usdt,
        taxes: TokenTaxes { buy_pct: Decimal::ZERO, sell_pct: Decimal::ZERO, transfer_pct: Decimal::ZERO },
        coin_dec: 18,
        usdt_dec: 18,
        entries: vec![pair],
        mids: vec![],
        gas: U256::ONE,
        usdt_ticker: "USDT".into(),
        smart: false,
    };
    let runtime = tokio::runtime::Handle::current();
    let (tx, rx) = mpsc::channel();
    let started = std::sync::Barrier::new(2);
    std::thread::scope(|scope| {
        let writer = q.reg.inner.write().unwrap();
        scope.spawn(|| {
            started.wait();
            let paths = runtime.block_on(fluid_hop_pricing::candidates(&q, &input, false));
            tx.send(paths.is_empty()).unwrap();
        });
        started.wait();
        let completed = rx.recv_timeout(Duration::from_secs(3));
        drop(writer);
        assert_eq!(completed, Ok(true));
    });
}
