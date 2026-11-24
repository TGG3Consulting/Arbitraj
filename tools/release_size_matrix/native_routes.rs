//! Optional CI lane: replay archived BSC states through the release's native router.
//! Pool RPC responses are identical. Amounts, tax/gas assumptions and routing
//! settings are controlled scenarios. No orders or RPC requests are sent.

#[path = "native_routes_common/mod.rs"]
mod route_common;
#[path = "native_routes_shopinx.rs"]
mod shopinx;

use alloy_primitives::U256;
use arb::core::types::{Asset, Quote, Side};
use arb::gateway::errors::GatewayResult;
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::quote::routing;
use rust_decimal::Decimal;
use serde_json::{Value, json};

async fn configure(parts: &Parts, mode: &str) {
    let scope = Scope::Venue("bsc_dex".into());
    let ur = "0x0000000000000000000000000000000000000099";
    for (name, value) in [
        ("dex.router_mode", json!(mode)),
        ("dex.swap_mode", json!("direct")),
        ("dex.intermediate_enabled", json!(true)),
        ("dex.route_max_hops", json!(3)),
        ("dex.route_max_paths", json!(50)),
        ("dex.route_max_pools_per_edge", json!(5)),
        ("smart.split_step_pct", json!("5")),
        ("smart.universal_router", json!(ur)),
        ("smart.permit2", json!(ur)),
        ("feed.stale_after_ms", json!(600_000)),
    ] {
        parts
            .ctx
            .settings
            .set(name, scope.clone(), Some(value), "native route replay")
            .await
            .unwrap_or_else(|e| panic!("setting {name}: {e:?}"));
    }
}

async fn cake_quote(parts: &Parts, side: Side, qty: Decimal) -> GatewayResult<Quote> {
    use route_common::{CAKE, USDT};
    let q = parts.quoting();
    let mids = q.mids.active(&q.ctx).into_iter().map(|m| (m, 18)).collect();
    let input = routing::Input {
        asset: Asset {
            chain: None,
            ticker: "CAKE".into(),
            contract: Some(CAKE),
        },
        side,
        qty,
        coin: CAKE,
        usdt: USDT,
        taxes: shopinx::zero(),
        coin_dec: 18,
        usdt_dec: 18,
        entries: q.reg.by_pair(CAKE, USDT),
        mids,
        gas: U256::from(shopinx::GAS),
        usdt_ticker: "USDT".into(),
        smart: false,
    };
    routing::run(q, input).await
}

fn emit(fixture: &str, block: u64, mode: &str, side: Side, amount: u32, result: GatewayResult<Quote>) {
    let value: Value = match result {
        Ok(quote) => json!({
            "fillable_qty": quote.fillable_qty.to_string(),
            "avg_price": quote.avg_price.to_string(),
            "route": quote.route,
            "costs": quote.costs,
        }),
        Err(error) => json!({"error_code": format!("{:?}", error.code)}),
    };
    println!(
        "NATIVE_ROUTE_MATRIX {}",
        json!({
            "fixture": fixture,
            "block": block,
            "router_mode": mode,
            "side": format!("{side:?}"),
            "requested_qty": amount,
            "result": value,
        }),
    );
}

#[tokio::test]
async fn native_route_release_matrix() {
    for (fixture, block) in [
        ("routes_2026-10-29", 130_521_896),
        ("routes_2026-10-30", 130_556_914),
    ] {
        let directory = format!("tests/data/{fixture}");
        let parts = route_common::parts_from(&directory).await;
        for mode in ["classic", "graph"] {
            configure(&parts, mode).await;
            for side in [Side::Buy, Side::Sell] {
                for amount in [1, 100, 1_000, 5_000, 20_000, 50_000, 100_000] {
                    let result = cake_quote(&parts, side, Decimal::from(amount)).await;
                    if mode == "classic" && amount == 1 {
                        assert!(result.is_ok(), "expected a small CAKE quote");
                    }
                    emit(fixture, block, mode, side, amount, result);
                }
            }
        }
    }

    let parts = shopinx::scene().await;
    for mode in ["classic", "graph"] {
        configure(&parts, mode).await;
        let quote = parts.quoting();
        for side in [Side::Buy, Side::Sell] {
            for amount in [1_000, 10_000, 28_000, 29_000, 30_000, 57_000] {
                let result = shopinx::full(&quote, side, Decimal::from(amount)).await;
                if mode == "classic" && amount == 1_000 {
                    assert!(result.is_ok(), "expected a small SHOPINX quote");
                }
                emit("fit_volume_2026-11-15", shopinx::BLOCK, mode, side, amount, result);
            }
        }
    }
}
