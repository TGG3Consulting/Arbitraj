use super::*;
use flate2::read::GzDecoder;
use serde_json::Value;

fn integer(v: &Value) -> U256 {
    U256::from_str_radix(&v.to_string(), 10).unwrap()
}

#[test]
fn local_amount_matches_each_original_well_in_its_two_directions() {
    let files: &[&[u8]] = &[
        include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/pinto_base_wells.json.gz"),
        include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/basin_ethereum_wells.json.gz"
        ),
    ];
    let mut counts = [0usize; 2];
    let mut discrepancies = Vec::new();
    for (n, gz) in files.iter().enumerate() {
        let archive: Value = serde_json::from_reader(GzDecoder::new(*gz)).unwrap();
        for (well, row) in archive["wells"].as_object().unwrap() {
            let pair = row["tokens"].as_array().unwrap();
            let addr = |i: usize| {
                pair[i]["token"]
                    .as_str()
                    .unwrap()
                    .parse::<Address>()
                    .unwrap()
            };
            let fn_data = match row["fnName"].as_str().unwrap() {
                "Constant Product 2" => BasinFunction::ConstantProduct2,
                "Stable2" => {
                    // Verified on this Base function's LUT, 0xba510a1eb7262853cdf910db2f6e4d1ee9457196.
                    assert_eq!(archive["chain"], 8453);
                    assert_eq!(
                        row["roles"]["function"]["address"],
                        "0xba51055a97b40d7f41f3f64b57469b5d45b67c87"
                    );
                    BasinFunction::Stable2 {
                        a: U256::from(100),
                        dec0: 6,
                        dec1: 6,
                    }
                }
                other => panic!("Unknown original Well function {other}"),
            };
            let data = BasinWellData {
                token0: addr(0),
                token1: addr(1),
                reserve0: integer(&pair[0]["reserve"]),
                reserve1: integer(&pair[1]["reserve"]),
                supply: integer(&row["supply"]),
                function: fn_data,
            };
            for q in row["quotes"].as_array().unwrap() {
                if !q["out"].is_number() {
                    continue;
                }
                counts[n] += 1;
                let i = q["direction"].as_u64().unwrap() as usize;
                let amount = integer(&q["amount"]);
                let expect = integer(&q["out"]);
                let got = swap_out(&data, addr(i), amount);
                if got != Some(expect) {
                    discrepancies.push(format!(
                        "{well} direction {i}, in={amount}, native={expect}, local={got:?}"
                    ));
                }
            }
        }
    }
    assert_eq!(counts[0], 30);
    assert!(counts[1] >= 30);
    assert!(
        discrepancies.is_empty(),
        "{} native/local differences: {}",
        discrepancies.len(),
        discrepancies.join("\n")
    );
}
