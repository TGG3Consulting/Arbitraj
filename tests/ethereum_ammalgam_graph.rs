//! Canonical state for the two markets used by both observed USDT settlement directions.
mod mid_dex_common;
use alloy_primitives::{Address, U256};
use arb::{
    core::types::{AmmalgamState, Asset, PoolState},
    settings::scope::Scope,
    venues::bsc_dex::{
        exec::executor::entry,
        parts::Parts,
        pools::{families, graph},
        quote::pool_quote,
    },
};
use serde_json::{Value, json};

fn read(bytes: &[u8]) -> Value {
    serde_json::from_reader(flate2::read::GzDecoder::new(bytes)).unwrap()
}
fn number(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}
fn address(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}

#[tokio::test]
async fn two_registered_edges_pay_the_locally_quoted_amount_in_both_directions() {
    let wallet = read(include_bytes!(
        "data/first_run_dex_2026-11-26/ammalgam_ethereum_wallet.json.gz"
    ));
    let quotes = read(include_bytes!(
        "data/first_run_dex_2026-11-26/ammalgam_ethereum_quotes.json.gz"
    ));
    assert_eq!(wallet["selector"], quotes["selector"]);
    let ctx = mid_dex_common::ctx("ethereum_ammalgam_edges", true).await;
    let family = families::make("ammalgam", "ammalgam_dlex").unwrap();
    let venue = Scope::Venue(ctx.place.id.into());
    for (setting, value) in [
        ("dex.evm_chain_id", json!(1)),
        ("dex.route_max_pools_per_edge", json!(4)),
    ] {
        ctx.settings
            .set(setting, venue.clone(), Some(value), "test")
            .await
            .unwrap();
    }
    for (setting, value) in [
        ("dex.router", wallet["module"]["address"].clone()),
        (
            "dex.module_runtime_hashes",
            json!([wallet["module"]["runtime"]]),
        ),
    ] {
        ctx.settings
            .set(setting, Scope::Dex(family.id.into()), Some(value), "test")
            .await
            .unwrap();
    }
    let p = Parts::new(ctx);
    let path = wallet["paths"][0]["tokens"].as_array().unwrap();
    let tokens = [address(&path[0]), address(&path[1]), address(&path[2])];
    p.tokens.set(
        &["WETH", "USDC", "USDT"]
            .iter()
            .zip(tokens)
            .map(|(t, a)| Asset {
                chain: None,
                ticker: (*t).into(),
                contract: Some(a),
            })
            .collect::<Vec<_>>(),
    );
    let mut entries = Vec::new();
    for (pair, segment) in wallet["paths"][0]["pairs"]
        .as_array()
        .unwrap()
        .iter()
        .zip(tokens.windows(2))
    {
        let mut e = mid_dex_common::pair(address(pair), segment[0], segment[1], "market", 1, 1);
        let rows = [e.pool.token0, e.pool.token1].map(|a| {
            quotes["rows"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["pool"] == *pair && address(&r["input"]) == a)
                .unwrap()
        });
        e.family = family;
        e.pool.family = family.id.into();
        e.pool.kind = family.kind;
        e.pool.state = PoolState::Ammalgam(Box::new(AmmalgamState {
            reserves: rows.map(|r| number(&r["state"][0])),
            reference: rows.map(|r| number(&r["state"][1])),
            missing: rows.map(|r| number(&r["state"][3])),
            cash: [
                number(&rows[1]["output_cash"]),
                number(&rows[0]["output_cash"]),
            ],
        }));
        p.registry.put(e.clone());
        assert!(entry::fits_ok(&p.ctx, &e));
        assert!(entry::route_fits(
            &p.ctx,
            &p.registry,
            family,
            &e.pool.reference
        ));
        entries.push(e);
    }
    let g = graph::build(&p.ctx, &p.registry, &p.tokens, 6).unwrap();
    for (i, path) in wallet["paths"].as_array().unwrap().iter().enumerate() {
        let mut amount = number(&path["spent"]);
        for (j, t) in path["tokens"].as_array().unwrap().windows(2).enumerate() {
            let a = address(&t[0]);
            let b = address(&t[1]);
            assert_eq!(g.between(a, b).len(), 1);
            let e = &entries[if i == 0 { j } else { 1 - j }];
            amount = pool_quote::swap(&p.ctx, e, a, true, amount)
                .unwrap()
                .amount_out;
        }
        assert_eq!(amount, number(&path["received"]));
    }
}
