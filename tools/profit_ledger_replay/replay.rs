//! Replay the posted cash flows separately from the receipt-verified offline adjustment.
use std::{collections::BTreeMap, env, fs};

use arb::core::{
    deal::{Deal, Step},
    deal_group::{DealGroup, PiecesDone},
    deal_return_cost,
    types::{Asset, Direction, VenueId},
    usdt_return_state::UsdtReturn,
};
use rust_decimal::Decimal;
use sha2::{Digest, Sha256};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
struct Row {
    groups: Vec<String>,
    has_end: bool,
    invested: Decimal,
    proceeds: Decimal,
    spent: Decimal,
    recorded: Option<Decimal>,
    return_cost: Option<Decimal>,
    first: Option<UsdtReturn>,
    rest: Vec<UsdtReturn>,
    pieces: Option<PiecesDone>,
    verified_credit: Option<Decimal>,
}

#[derive(Default, Serialize)]
struct Stats {
    count: usize,
    nonzero_investment: usize,
    end_unavailable: usize,
    known_pnl: usize,
    recorded: Decimal,
    recomputed_from_fills: Decimal,
    verified_corrections: usize,
    after_verified_receipts: Decimal,
    material_ledger_disagreements: usize,
    maximum_absolute_difference: Decimal,
}

#[derive(Deserialize)]
struct Capture {
    original: Vec<Row>,
    latest: Vec<Row>,
}

#[test]
fn all_observed_deals() {
    let path = env::var("PRIVATE_LEDGER_INPUT").unwrap();
    let capture: Capture = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let datasets = [("original", capture.original), ("latest", capture.latest)];
    let mut output = BTreeMap::new();
    for (dataset, rows) in datasets {
        assert!(!rows.is_empty());
        let mut ordered = Sha256::new();
        let mut summaries: BTreeMap<String, Stats> = BTreeMap::new();
        for row in rows {
            let direction = Direction {
                asset: Asset { ticker: "unit".into(), chain: None, contract: None },
                buy_on: VenueId("source".into()), sell_on: VenueId("destination".into()),
            };
            let mut d = Deal::new(String::new(), direction, serde_json::Value::Null, Decimal::ZERO);
            d.step = Step::Closed;
            d.invested = row.invested;
            d.proceeds = row.proceeds;
            d.spent = row.spent;
            d.usdt_return = row.first;
            d.usdt_return_parts = row.rest;
            if let Some(pieces) = row.pieces {
                let mut group = DealGroup::new("");
                group.pieces = Some(pieces);
                d.group = Some(Box::new(group));
            }
            d.result_usd = row.recorded.map(|_| d.result_if(Decimal::ZERO));
            d.return_cost_usd = None;
            deal_return_cost::apply(&mut d);
            let repeated = deal_return_cost::apply(&mut d);
            assert_eq!(repeated, Decimal::ZERO, "repeated replay charged a return");
            let reconstructed = d.result_usd;
            // The archived return remains posted until an operator performs a separate reconciliation.
            d.result_usd = row.recorded;
            d.return_cost_usd = row.return_cost;
            if let Some(new_credit) = row.verified_credit {
                match d.usdt_return.as_mut() {
                    Some(UsdtReturn::Done { credited, .. }) => *credited = new_credit,
                    _ => panic!("verified receipt without a completed return"),
                }
                deal_return_cost::apply(&mut d);
                assert_eq!(deal_return_cost::apply(&mut d), Decimal::ZERO);
            }
            for value in [row.recorded, reconstructed, d.result_usd] {
                match value {
                    Some(v) => ordered.update(v.normalize().to_string().as_bytes()),
                    None => ordered.update(b"missing"),
                }
                ordered.update([0u8]);
            }
            for group in row.groups {
                let s = summaries.entry(group).or_default();
                s.count += 1;
                s.nonzero_investment += usize::from(!d.invested.is_zero());
                s.end_unavailable += usize::from(!row.has_end);
                if let (Some(recorded), Some(calc), Some(adjusted)) = (row.recorded, reconstructed, d.result_usd) {
                    let difference = (recorded - calc).abs();
                    s.known_pnl += 1;
                    s.recorded += recorded;
                    s.recomputed_from_fills += calc;
                    s.verified_corrections += usize::from(row.verified_credit.is_some());
                    s.after_verified_receipts += adjusted;
                    s.maximum_absolute_difference = s.maximum_absolute_difference.max(difference);
                    s.material_ledger_disagreements += usize::from(difference > Decimal::new(1, 12));
                }
            }
        }
        assert_eq!(summaries["all"].verified_corrections, 2, "TLOS receipt coverage");
        let digest = hex::encode(ordered.finalize());
        output.insert(dataset, serde_json::json!({"periods": summaries, "row_digest": digest}));
    }
    fs::write(env::var("LEDGER_RESULT").unwrap(), serde_json::to_vec_pretty(&output).unwrap()).unwrap();
}
