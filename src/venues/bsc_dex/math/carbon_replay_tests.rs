use super::*;
use crate::core::types::CarbonStrategy;
use flate2::read::GzDecoder;
use serde_json::{Value, json};

fn number<T: std::str::FromStr>(v: &Value) -> T
where
    T::Err: std::fmt::Debug,
{
    v.as_str().unwrap().parse().unwrap()
}

fn market(raw: &Value, controller: Address) -> CarbonState {
    let tokens: [Address; 2] = [number(&raw["tokens"][0]), number(&raw["tokens"][1])];
    let strategies = raw["orders"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            let mut orders = std::array::from_fn(|i| {
                let o = &r[3][i];
                CarbonOrder {
                    y: number(&o[0]),
                    z: number(&o[1]),
                    a: number(&o[2]),
                    b: number(&o[3]),
                }
            });
            let physical: [Address; 2] = [number(&r[2][0]), number(&r[2][1])];
            if physical != tokens {
                assert_eq!(physical, [tokens[1], tokens[0]]);
                orders.swap(0, 1);
            }
            CarbonStrategy {
                id: number(&r[0]),
                orders,
            }
        })
        .collect();
    CarbonState {
        controller,
        tokens,
        native_side: tokens.iter().position(|a| *a == Address::repeat_byte(0xee)),
        fee_ppm: raw["fee_ppm"].as_u64().unwrap().try_into().unwrap(),
        strategies,
    }
}

fn actions_json(p: &Plan) -> Value {
    json!({"input":p.spent.to_string(),"output":p.received.to_string(),"fee":p.fee.to_string(),
        "actions":p.actions.iter().map(|a|[a.id.to_string(),a.amount.to_string()]).collect::<Vec<_>>()})
}

#[test]
fn recorded_order_vectors_preserve_or_improve_all_reference_fills() {
    let bytes =
        include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/carbon_ethereum.json.gz");
    let data: Value = serde_json::from_reader(GzDecoder::new(bytes.as_slice())).unwrap();
    let mut cases = Vec::new();
    for c in data["controls"].as_array().unwrap() {
        let source = number::<Address>(&c["source"]);
        let target = number::<Address>(&c["target"]);
        let row = data["directory"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| {
                let tokens = r["tokens"].as_array().unwrap();
                tokens.contains(&c["source"]) && tokens.contains(&c["target"])
            })
            .unwrap();
        let s = market(row, number(&data["roles"][0]));
        assert!(s.tokens.contains(&target));
        let by_target = c["by_target"].as_bool().unwrap();
        let spent = number::<u128>(&c["input"]);
        let received = number::<u128>(&c["output"]);
        let size = if by_target { received } else { spent };
        let plans = alternatives(&s, source, by_target, size);
        assert!(!plans.is_empty());
        assert!(if by_target {
            plans[0].spent <= spent
        } else {
            plans[0].received >= received
        });
        let rows: Vec<_> = plans
            .iter()
            .map(|p| {
                assert_eq!(p.actions.iter().map(|a| a.amount).sum::<u128>(), size);
                assert!(after(&s, source, p).is_some());
                actions_json(p)
            })
            .collect();
        cases.push(json!({"source":c["source"],"target":c["target"],"by_target":by_target,"amount":size.to_string(),
            "reference_id":c["id"],"reference_input":c["input"],"reference_output":c["output"],"alternatives":rows}));
    }
    assert_eq!(cases.len(), 12);
    // Four independent two-strategy controls also exercise configurations where a
    // single order wins after paying the extra gas of updating another strategy.
    let registry: Value = serde_json::from_str(include_str!(
        "../../../../tests/data/first_run_dex_2026-11-26/carbon_registry_ethereum.json"
    ))
    .unwrap();
    assert_eq!(registry["selector"], data["selector"]);
    for c in registry["multi_quotes"].as_array().unwrap() {
        let source = number::<Address>(&c["source"]);
        let target = number::<Address>(&c["target"]);
        let row = data["directory"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| {
                let t = r["tokens"].as_array().unwrap();
                t.contains(&c["source"]) && t.contains(&c["target"])
            })
            .unwrap();
        let s = market(row, number(&data["roles"][0]));
        let by_target = c["by_target"].as_bool().unwrap();
        let size = c["actions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| number::<u128>(&a[1]))
            .sum::<u128>();
        let amount = u128::from_str_radix(
            c["aggregate"]["result"]
                .as_str()
                .unwrap()
                .trim_start_matches("0x"),
            16,
        )
        .unwrap();
        let (spent, received) = if by_target {
            (amount, size)
        } else {
            (size, amount)
        };
        let plans = alternatives(&s, source, by_target, size);
        let rows = plans.iter().map(actions_json).collect::<Vec<_>>();
        assert!(!rows.is_empty() && s.tokens.contains(&target));
        cases.push(json!({"source":c["source"],"target":c["target"],"by_target":by_target,"amount":size.to_string(),
            "reference_actions":c["actions"],"reference_input":spent.to_string(),"reference_output":received.to_string(),"alternatives":rows}));
    }
    assert_eq!(cases.len(), 16);
    if let Some(path) = std::env::var_os("ARB_CARBON_ALLOCATION_EXPORT") {
        let replay =
            json!({"selector":data["selector"],"state_root":data["state_root"],"cases":cases});
        std::fs::write(path, serde_json::to_vec(&replay).unwrap()).unwrap();
    }
}
