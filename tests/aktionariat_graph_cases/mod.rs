//! Входы штатного графа из того же блока, на котором записаны транзакции V5.
use super::*;
use arb::core::types::BrokerbotState;
use arb::venues::bsc_dex::{
    check::{emitter, simulate::SimLog},
    exec::{
        chain::Step,
        executor::{call, entry},
        plan::Order,
    },
    pools::ticks::{TickData, TickMap},
    quote::{pool_after, pool_quote, route_paths},
};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};

mod data;
use data::*;

#[tokio::test]
async fn three_hop_graph_matches_both_whole_share_payments_and_original_executor_bytes() {
    let tape = fixture(include_bytes!(
        "../data/first_run_dex_2026-11-26/aktionariat_ethereum_usdt.json.gz"
    ));
    let directory = fixture(include_bytes!(
        "../data/first_run_dex_2026-11-26/aktionariat_ethereum_directory.json.gz"
    ));
    let curves: Value = serde_json::from_slice(include_bytes!(
        "../data/first_run_dex_2026-11-26/aktionariat_ethereum_curves.json"
    ))
    .unwrap();
    let module = addr(&tape, "module");
    let exec = addr(&tape, "executor");
    let cashes: HashSet<_> = tape["controls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| addr(r, "cash"))
        .collect();
    let usd = address!("dac17f958d2ee523a2206206994597c13d831ec7");
    let w = address!("c02aaa39b223fe8d0a0e5c4f27ead9083c756cc2");
    let mut allowed = cashes.clone();
    allowed.insert(w);
    let mut ctx = ctx(91).await;
    ctx.place = arb::venues::bsc_dex::ctx::Place {
        id: "eth_dex",
        all: &["eth_dex"],
    };
    let scope = Scope::Venue("eth_dex".into());
    for (name, val) in [
        ("venue.quote_asset", json!("USDT")),
        ("dex.chain", json!("ethereum")),
        ("dex.evm_chain_id", json!(1)),
        ("dex.router_mode", json!("graph")),
        ("dex.route_max_hops", json!(3)),
        ("dex.intermediate_enabled", json!(true)),
        ("dex.intermediate_tokens", json!(allowed)),
        ("dex.native_wrapped", json!(w)),
        ("dex.depth_band_pct", json!(1)),
        ("dex.min_pool_liquidity_usd", json!(1)),
        ("dex.depth_hysteresis_pct", json!(0)),
        ("dex.route_max_pools_per_edge", json!(4)),
        ("dex.deadline_s", json!(900)),
    ] {
        ctx.settings
            .set(name, scope.clone(), Some(val), "тест")
            .await
            .unwrap();
    }
    let slot = Scope::Dex("aktionariat-ethereum".into());
    for (key, v) in [
        ("dex.router", json!(module)),
        (
            "dex.module_runtime_hashes",
            json!(["0xc89be9fbbce19d4f938d94776d4eec71574a276bc61bf8bbe8ff158a670857d3"]),
        ),
    ] {
        ctx.settings
            .set(key, slot.clone(), Some(v), "тест")
            .await
            .unwrap();
    }
    let p = Parts::new(ctx);
    p.tokens.set(&[Asset {
        chain: Some(arb::core::types::ChainId("ethereum".into())),
        ticker: "USDT".into(),
        contract: Some(usd),
    }]);
    for r in tape["controls"].as_array().unwrap() {
        let broker = addr(r, "broker");
        let chf = addr(r, "cash");
        let stock = addr(r, "share");
        let units = U256::from(r["units"].as_u64().unwrap());
        let key = r["broker"].as_str().unwrap();
        let c = curves["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["broker"] == key)
            .unwrap();
        let shares = market(broker, &directory["markets"][key], c);
        p.registry.put(shares.clone());
        let mut markets = vec![];
        for (i, (coin, counter)) in [(w, usd), (chf, w)].into_iter().enumerate() {
            let pool = r["pools"][i].as_str().unwrap();
            let e = bridge_pool(pool.parse().unwrap(), coin, counter, &tape["states"][pool]);
            p.registry.put(e.clone());
            markets.push(e);
        }
        markets.push(shares);
        assert_eq!(p.tokens.usdt(&p.ctx).unwrap(), usd);
        for e in &markets {
            let d = p
                .registry
                .depth()
                .depth(&p.ctx, &p.registry, &p.tokens, e, 6);
            assert!(
                d.as_ref().is_ok_and(|x| x.deep_buy || x.deep_sell),
                "ожидалось ребро {:?}, глубина {:?}, chain {:?}",
                e.pool.reference,
                d,
                p.ctx.chain_id()
            );
        }
        let g = graph::build(&p.ctx, &p.registry, &p.tokens, 6).unwrap();
        let path = route_paths::enumerate(&g, usd, stock, &allowed, 3, 20)
            .into_iter()
            .find(|p| p.hops() == 3)
            .expect("зарегистрированы три хопа");
        assert!(route_paths::enumerate(&g, usd, stock, &HashSet::from([w]), 3, 20).is_empty());
        let mut qty = units;
        for h in path.steps.iter().rev() {
            let fill = pool_quote::swap(&p.ctx, &h.edge.entry, h.token_in, false, qty).unwrap();
            assert!(fill.complete);
            qty = fill.amount_in;
        }
        assert_eq!(qty, number(r, "budget"));
        for (index, (buy, from, minimum)) in
            [(true, qty, units), (false, units, number(r, "returned"))]
                .into_iter()
                .enumerate()
        {
            if !buy {
                markets.reverse();
            }
            let mut token = if buy { usd } else { stock };
            let mut amount = from;
            let mut steps = vec![];
            for (i, e) in markets.iter_mut().enumerate() {
                assert!(entry::fits_ok(&p.ctx, e));
                let output = if token == e.pool.token0 {
                    e.pool.token1
                } else {
                    e.pool.token0
                };
                let (swapped, next) = pool_after::apply(&p.ctx, e, token, amount).unwrap();
                assert!(swapped.complete);
                steps.push(Step {
                    entry: e.clone(),
                    from_wallet: i == 0,
                    to_wallet: i == 2,
                    order: Order {
                        token_in: token,
                        token_out: output,
                        amount_in: amount,
                        min_out: if i == 2 { minimum } else { U256::ONE },
                    },
                });
                e.pool.state = next;
                amount = swapped.amount_out;
                token = output;
            }
            assert_eq!(amount, minimum);
            let logs: Vec<SimLog> = serde_json::from_value(r["logs"][index].clone()).unwrap();
            let registered: Vec<_> = steps.iter().map(|s| &s.entry).collect();
            assert!(emitter::missing(&registered, &logs).is_empty());
            let wire = &r[if buy { "buy" } else { "sell" }]["data"]
                .as_str()
                .unwrap()[2..];
            let deadline = u64::from_str_radix(&wire[8 + 4 * 64..8 + 5 * 64], 16).unwrap();
            let data = call::build(&p.ctx, &[steps], exec, deadline - 900)
                .unwrap()
                .data();
            assert_eq!(alloy_primitives::hex::encode(data), wire);
        }
    }
}
