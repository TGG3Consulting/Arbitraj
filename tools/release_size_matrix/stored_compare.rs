//! Native fee and size calculation over two saved pre-decision CEX ladders.
//! Pair rules, network precision/minimums and the historical balance are absent.
//! k, h, closure share and profit floors are pinned for cross-release comparison.

use arb::core::card_max::passes;
use arb::core::dirctx::DirCtx;
use arb::core::size::{Curve, Params};
use arb::core::size_curve::{SourceCurve, cumulative};
use arb::core::size_search::search;
use arb::core::types::{Asset, Direction, OrderBook, PriceLevel, VenueId};
use arb::core::uncapped_recompute_cfg::{presence, step_of};
use arb::core::uncapped_replay::{Plan, Replay};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use std::str::FromStr;

fn decimal(v: &Value) -> Result<Decimal, String> {
    let s = v.as_str().ok_or("missing decimal string")?;
    Decimal::from_str(s).map_err(|e| e.to_string())
}

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn context(dir: &Direction, plan: &Plan) -> DirCtx {
    let (qa, qb) = ("USDT".to_string(), "USDT".to_string());
    let (k, h, closure_share) = (d("3.6"), d("0.7"), Some(d("0.8")));
    // NATIVE_DIR_CTX
}

fn params(
    plan: &Plan,
    book: &OrderBook,
    sell: &OrderBook,
    budget: Decimal,
    floor: (Decimal, Decimal),
    max_evals: u32,
) -> Params {
    // NATIVE_EXTRA
    Params {
        // NATIVE_PARAMS
    }
}

fn convert_asset(value: &mut Value) {
    let fields = value.as_object_mut().expect("archived asset is an object");
    if !fields.contains_key("bsc_contract") && let Some(addr) = fields.remove("contract") {
        fields.insert("bsc_contract".into(), addr);
    }
    let expected = value["bsc_contract"].as_str().expect("positive historical contract required");
    let parsed: Asset = serde_json::from_value(value.clone()).expect("native asset structure");
    let actual = parsed.contract.expect("preserve original ERC-20 contract in Rust");
    assert_eq!(format!("{actual:#x}"), expected.to_ascii_lowercase());
}

fn read_book(v: &Value, dir: &Direction, buy: bool) -> Result<OrderBook, String> {
    let levels = v["levels"].as_array().ok_or("missing book levels")?;
    let levels: Vec<PriceLevel> = levels
        .iter()
        .map(|p| {
            Ok(PriceLevel {
                price: decimal(&p[0])?,
                qty: decimal(&p[1])?,
            })
        })
        .collect::<Result<_, String>>()?;
    if levels.is_empty() {
        return Err("empty book levels".into());
    }
    let (venue, bids, asks): (VenueId, _, _) = if buy {
        (dir.buy_on.clone(), vec![], levels)
    } else {
        (dir.sell_on.clone(), levels, vec![])
    };
    Ok(OrderBook {
        venue,
        asset: dir.asset.clone(),
        bids,
        asks,
        exchange_time: None,
        received_at: DateTime::<Utc>::from_timestamp_nanos(
            v["recv_ns"].as_i64().ok_or("no receive time")?,
        ),
    })
}

async fn replay(case: &Value, budget: Decimal, max_evals: u32) -> Result<Value, String> {
    let mut direction = case["direction"].clone();
    let mut plan = case["plan"].clone();
    convert_asset(&mut direction["asset"]);
    convert_asset(&mut plan["buy_quote"]["asset"]);
    convert_asset(&mut plan["sell_quote"]["asset"]);
    let dir: Direction = serde_json::from_value(direction).map_err(|e| e.to_string())?;
    let plan = Plan::parse(&plan)?;
    let a = read_book(&case["buy"], &dir, true)?;
    let b = read_book(&case["sell"], &dir, false)?;
    let source = Replay::new(&dir, &plan, a.clone(), Some(b.clone()), ["USDT", "USDT"])?;
    let cx = context(&dir, &plan);
    let floor = (d("0.05"), Decimal::ZERO);
    let mut curve = SourceCurve {
        cx: &cx,
        src: &source,
    };
    let reference = curve.at(plan.qty).await;
    let p = params(&plan, &a, &b, budget, floor, max_evals);
    let actual = search(&mut curve, &p).await;
    let best = actual.as_ref().ok().map(|f| &f.best);
    let own = reference.as_ref().ok().map(|point| &point.data);
    Ok(json!({
        "own_qty": plan.qty, "same_inputs_own_profit_usdt": own.map(|p| p.profit_usd),
        "own_cost_breakdown": own.map(|p| json!({
            "buy_average": p.buy_avg, "sell_average": p.sell_avg, "sell_qty": p.sell_qty,
            "invested_usdt": p.invested, "proceeds_usdt": p.proceeds, "costs_usdt": p.costs_usdt,
            "costs": p.costs,
        })) ,
        "search_error": actual.as_ref().err().map(|e| format!("{e:?}")),
        "selected_qty": best.map(|p| p.qty), "selected_spend_max_usdt": best.map(|p| p.invested),
        "selected_profit_usdt": best.map(|p| p.profit),
        "passes": best.is_some_and(|p| passes(&p.data, floor.0, floor.1)),
        "threshold_placement": "NATIVE_THRESHOLD_PLACEMENT",
    }))
}

#[tokio::test]
async fn stored_trade_matrix() {
    let cases: Vec<Value> = serde_json::from_str(include_str!("size_stored_cases.json")).unwrap();
    for case in &cases {
        let budget =
            decimal(&case["plan"]["size"]["budget"]).expect("case plan has a recorded budget");
        for max_evals in [300, 5000] {
            let result = match replay(case, budget, max_evals).await {
                Ok(metrics) => metrics,
                Err(reason) => json!({"unavailable": reason}),
            };
            println!(
                "STORED_TRADE_MATRIX {}",
                json!({
                    "deal_id": case["deal_id"], "window_id": case["window_id"],
                    "basis": case["basis"], "decision_utc": case["decision_utc"],
                    "decision_time_basis": case["decision_time_basis"], "source_release": case["source_release"],
                    "recorded_plan_budget_usdt": case["plan"]["size"]["budget"],
                    "budget_usdt": budget, "observed_profit_usdt": case["historical_result_usdt"],
                    "model": {"k": "3.6", "h": "0.7", "closure_share": "0.8", "min_usdt": "0.05", "min_pct": "0", "eval_limit": max_evals},
                    "result": result,
                })
            );
        }
    }
}
