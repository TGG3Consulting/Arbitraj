use super::{pair, route_common};
use alloy_primitives::{Address, U256};
use arb::core::types::{Asset, CheckState, CoinCheck, PoolRef, Side, TokenTaxes, VenueId};
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::{parts::Parts, pools::graph, quote::routing};
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use std::time::Instant;

fn amount(v: &Value) -> Decimal {
    v.as_str().unwrap().parse().unwrap()
}

fn buy_cost(asks: &Value, qty: Decimal) -> Option<Decimal> {
    let mut left = qty;
    let mut cost = Decimal::ZERO;
    for level in asks.as_array()? {
        let take = amount(&level[1]).min(left);
        cost += take * amount(&level[0]);
        left -= take;
        if left.is_zero() {
            return Some(cost);
        }
    }
    None
}

#[tokio::test]
async fn recorded_bsc_depth_and_pool_limits() {
    let snap: Value =
        serde_json::from_str(include_str!("../data/bsc_v2_policy_2026_11_25/block.json")).unwrap();
    let usdt: Address = snap["quote_token"].as_str().unwrap().parse().unwrap();
    let pools = snap["pools"].as_array().unwrap();
    let mut rows = Vec::new();
    for ticker in ["BNB", "USDC"] {
        let coin: Address = snap["tokens"][ticker].as_str().unwrap().parse().unwrap();
        let asset = Asset {
            ticker: ticker.into(),
            contract: Some(coin),
            chain: None,
        };
        let taxes = TokenTaxes {
            buy_pct: Decimal::ZERO,
            sell_pct: Decimal::ZERO,
            transfer_pct: Decimal::ZERO,
        };
        for (cap, band) in [(10, 20), (15, 20), (30, 20), (30, 70)] {
            let p = Parts::new(route_common::ctx().await);
            let scope = Scope::Venue("bsc_dex".into());
            for (k, v) in [
                ("dex.route_max_pools_per_edge", json!(cap)),
                ("dex.depth_band_pct", json!(band)),
                ("dex.depth_hysteresis_pct", json!(10)),
                ("dex.min_pool_liquidity_usd", json!(10)),
                ("dex.tick_read_band_pct", json!(70)),
                ("dex.router_mode", json!("graph")),
            ] {
                p.ctx
                    .settings
                    .set(k, scope.clone(), Some(v), "replay")
                    .await
                    .unwrap();
            }
            p.tokens.set(&[
                asset.clone(),
                Asset {
                    ticker: "USDT".into(),
                    contract: Some(usdt),
                    chain: None,
                },
            ]);
            for token in [coin, usdt] {
                p.tokens.remember_decimals(token, 18);
            }
            p.checks.set_coin(CoinCheck {
                venue: VenueId("bsc_dex".into()),
                asset: asset.clone(),
                taxes: CheckState::Known(taxes.clone()),
                source: "model".into(),
                checked_at: Some(Utc::now()),
                wait: None,
            });
            for row in pools.iter().filter(|r| r["coin"] == ticker) {
                let addr = row["address"].as_str().unwrap().parse().unwrap();
                let mut e = pair(
                    addr,
                    coin,
                    usdt,
                    row["reserve_coin"].as_str().unwrap().parse().unwrap(),
                    row["reserve_usdt"].as_str().unwrap().parse().unwrap(),
                );
                e.pool.fee_pips =
                    Some(u32::try_from(amount(&row["fee_pct"]) * Decimal::from(10000)).unwrap());
                e.pool.block = snap["block"].as_u64().unwrap();
                let reference = e.pool.reference.clone();
                p.registry.put(e);
                p.checks.update_pool(&reference, |f| {
                    f.gas_buy = Some(120000);
                    f.gas_sell = Some(120000);
                    f.quoter.buy = Some(Ok(()));
                    f.quoter.sell = Some(Ok(()));
                    f.taxes = Some(route_common::clean_taxes());
                });
            }
            let start = Instant::now();
            let graph = graph::build(&p.ctx, &p.registry, &p.tokens, 18).unwrap();
            let build_us = start.elapsed().as_micros();
            let mut kept: Vec<_> = graph
                .between(coin, usdt)
                .into_iter()
                .map(|e| match e.entry.pool.reference {
                    PoolRef::Contract(a) => a.to_string(),
                    _ => unreachable!(),
                })
                .collect();
            kept.sort();
            assert!(!kept.is_empty() && kept.len() <= cap);
            let book = &snap["books"][ticker]["asks"];
            // Sale after a recorded MEXC withdrawal of BNB. USDC tariff is an explicit sensitivity assumption.
            let (qty, withdraw, remaining_cost) = if ticker == "BNB" {
                (
                    Decimal::new(2486, 4),
                    Decimal::new(1, 5),
                    Decimal::from_str_exact("0.0093506921025175635435125468").unwrap(),
                )
            } else {
                (Decimal::from(100), Decimal::ZERO, Decimal::new(1, 2))
            };
            let cex = buy_cost(book, qty).expect("archived book must cover the whole quantity");
            let tradable = qty - withdraw;
            let q = p.quoting();
            let input = routing::Input {
                asset: asset.clone(),
                side: Side::Sell,
                qty: tradable,
                coin,
                usdt,
                taxes: taxes.clone(),
                coin_dec: 18,
                usdt_dec: 18,
                entries: p.registry.by_pair(coin, usdt),
                mids: vec![],
                gas: U256::from(
                    snap["gas_price_wei"]
                        .as_str()
                        .unwrap()
                        .parse::<u64>()
                        .unwrap(),
                ),
                usdt_ticker: "USDT".into(),
                smart: true,
            };
            let start = Instant::now();
            let quote = routing::run(q, input)
                .await
                .expect("full sell quote on the recorded V2 market");
            let quote_us = start.elapsed().as_micros();
            assert_eq!(quote.fillable_qty, tradable);
            let gross = quote.avg_price * quote.fillable_qty;
            let net = gross - cex * (Decimal::ONE + Decimal::new(5, 4)) - remaining_cost;
            rows.push(json!({"coin":ticker,"cap":cap,"band":band,"pool_count":kept.len(),"pools":kept,
                "build_us":build_us,"quote_us":quote_us,"bought":qty,"sell_qty":tradable,"cex_gross":cex,
                "dex_gross":gross,"cost_beyond_trade_fees_usdt":remaining_cost,"conditional_net_usdt":net,"route":quote.route}));
        }
    }
    assert_eq!(rows.len(), 8);
    assert!(
        rows.iter()
            .any(|r| r["cap"] == 30 && r["pool_count"].as_u64().unwrap() > 15)
    );
    if let Some(file) = std::env::var_os("GRAPH_POLICY_RESULT") {
        std::fs::write(file, serde_json::to_vec_pretty(&rows).unwrap()).unwrap();
    }
}
