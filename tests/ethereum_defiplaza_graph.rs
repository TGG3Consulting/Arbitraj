//! Приём двух зарегистрированных касс в штатный граф и пересечение их денежных остатков.
mod mid_dex_common;
use alloy_primitives::{Address, U256};
use arb::{
    core::types::{
        Asset, PoolRef, PoolState,
        defiplaza::{DefiPlazaCurve, DefiPlazaState},
    },
    settings::scope::Scope,
    venues::bsc_dex::{
        exec::executor::entry,
        parts::Parts,
        pools::{families, graph, registry::Entry},
        quote::pool_quote,
    },
};
use serde_json::{Value, json};

fn book(bytes: &[u8]) -> Value {
    serde_json::from_reader(flate2::read::GzDecoder::new(bytes)).unwrap()
}
fn pool(book: &Value, wallet: &Value, base: Address, counter: Address) -> Entry {
    let vault = book["pool"].as_str().unwrap().parse().unwrap();
    let mut e = mid_dex_common::pair(vault, base, counter, "test", 1, 1);
    let tokens = [e.pool.token0, e.pool.token1];
    let inv = &book["inventory"];
    let stable = book["kind"] == "stable";
    let kind = if stable {
        "defiplaza_stable"
    } else {
        "defiplaza_multi"
    };
    let family = families::make(kind, kind).unwrap();
    let assets = inv["assets"].as_array().unwrap();
    let index = tokens.map(|a| {
        assets
            .iter()
            .position(|v| v.as_str().unwrap().parse::<Address>().unwrap() == a)
            .unwrap()
    });
    let balances = tokens.map(|a| {
        inv["balances"][format!("{a:#x}")]
            .as_str()
            .unwrap()
            .parse::<U256>()
            .unwrap()
    });
    let values =
        |key: &str| index.map(|i| U256::from(inv["registration"][key][i].as_u64().unwrap()));
    let (curve, factors, reserves) = if stable {
        (
            DefiPlazaCurve::Stable {
                fee_bps: inv["config"][1].as_u64().unwrap().try_into().unwrap(),
                delta: inv["config"][6].as_u64().unwrap(),
            },
            values("denorm"),
            values("reserves"),
        )
    } else {
        (
            DefiPlazaCurve::Multi {
                one_minus_fee: inv["config"][1].as_u64().unwrap(),
            },
            [U256::ONE; 2],
            balances,
        )
    };
    e.family = family;
    e.pool.family = family.id.into();
    e.pool.kind = family.kind;
    e.pool.reference = PoolRef::Multi {
        contract: vault,
        token0: tokens[0],
        token1: tokens[1],
    };
    e.pool.state = PoolState::DefiPlaza(Box::new(DefiPlazaState {
        pool: vault,
        module: wallet["module"].as_str().unwrap().parse().unwrap(),
        module_hash: wallet["runtime"].as_str().unwrap().parse().unwrap(),
        tokens,
        physical: tokens,
        balances,
        reserves,
        factors,
        curve,
    }));
    e
}
#[tokio::test]
async fn funded_depth_and_shared_tokens_determine_the_graph_and_usable_v5_target() {
    let stable = book(include_bytes!(
        "data/first_run_dex_2026-11-26/defiplaza_stable_ethereum.json.gz"
    ));
    let multi = book(include_bytes!(
        "data/first_run_dex_2026-11-26/defiplaza_multi_ethereum.json.gz"
    ));
    let protected_stable = book(include_bytes!(
        "data/first_run_dex_2026-11-26/defiplaza_stable_protected_ethereum.json.gz"
    ));
    let protected_multi = book(include_bytes!(
        "data/first_run_dex_2026-11-26/defiplaza_multi_protected_ethereum.json.gz"
    ));
    let assets = stable["inventory"]["assets"].as_array().unwrap();
    let [tusd, dai, usdc, usdt] =
        std::array::from_fn(|i| assets[i].as_str().unwrap().parse::<Address>().unwrap());
    let ctx = mid_dex_common::ctx("plaza_graph", true).await;
    let p = Parts::new(ctx);
    p.ctx
        .settings
        .set(
            "dex.route_max_pools_per_edge",
            Scope::Venue("bsc_dex".into()),
            Some(json!(4)),
            "test",
        )
        .await
        .unwrap();
    p.tokens.set(&[
        Asset {
            chain: None,
            ticker: "USDC".into(),
            contract: Some(usdc),
        },
        Asset {
            chain: None,
            ticker: "USDT".into(),
            contract: Some(usdt),
        },
    ]);
    let first = pool(&stable, &protected_stable, usdc, usdt);
    let small = pool(&multi, &protected_multi, usdc, usdt);
    for (e, snap, proof) in [
        (&first, &stable, &protected_stable),
        (&small, &multi, &protected_multi),
    ] {
        let scope = Scope::Dex(e.family.id.into());
        for (k, v) in [
            ("dex.factory", snap["pool"].clone()),
            ("dex.router", proof["module"].clone()),
            ("dex.module_runtime_hashes", json!([proof["runtime"]])),
        ] {
            p.ctx
                .settings
                .set(k, scope.clone(), Some(v), "test")
                .await
                .unwrap();
        }
        p.registry.put(e.clone());
        assert!(entry::fits_ok(&p.ctx, e));
        assert!(entry::route_fits(
            &p.ctx,
            &p.registry,
            e.family,
            &e.pool.reference
        ));
    }
    let size = U256::from(1_000_000);
    assert_eq!(
        pool_quote::swap(&p.ctx, &first, usdc, true, size)
            .unwrap()
            .amount_out,
        U256::from(1_000_520)
    );
    assert_eq!(
        pool_quote::swap(&p.ctx, &small, usdc, true, size)
            .unwrap()
            .amount_out,
        U256::from(147)
    );
    assert!(pool_quote::swap(&p.ctx, &small, usdc, false, size).is_err());
    assert!(first.shares_liquidity(&pool(&stable, &protected_stable, usdc, dai)));
    assert!(
        !first.shares_liquidity(&pool(&stable, &protected_stable, tusd, dai))
            && !first.shares_liquidity(&small)
    );
    let g = graph::build(&p.ctx, &p.registry, &p.tokens, 6).unwrap();
    let edges = g.between(usdc, usdt);
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].entry.pool.reference, first.pool.reference);
    let mut funded = small.clone();
    let PoolState::DefiPlaza(ref mut market) = funded.pool.state else {
        unreachable!()
    };
    market.balances = [U256::from(5_000_000_000u64); 2];
    market.reserves = market.balances;
    funded.pool.block += 1;
    funded.synced_block = funded.pool.block;
    p.registry.put(funded);
    let refreshed = graph::build(&p.ctx, &p.registry, &p.tokens, 6).unwrap();
    assert_eq!(refreshed.between(usdc, usdt).len(), 2);
    p.ctx
        .settings
        .set(
            "dex.module_runtime_hashes",
            Scope::Dex(first.family.id.into()),
            Some(json!([format!(
                "{:#x}",
                alloy_primitives::B256::repeat_byte(7)
            )])),
            "test",
        )
        .await
        .unwrap();
    assert!(!entry::route_fits(
        &p.ctx,
        &p.registry,
        first.family,
        &first.pool.reference
    ));
}
