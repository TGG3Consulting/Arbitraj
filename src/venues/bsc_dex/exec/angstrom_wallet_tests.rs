use super::*;
use flate2::read::GzDecoder;
use serde_json::Value;

#[test]
fn protected_receipt_binds_the_original_user_key_height_and_both_balances() {
    let raw = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/angstrom_ethereum_wallets.json.gz"
    );
    let a: Value = serde_json::from_reader(GzDecoder::new(&raw[..])).unwrap();
    let d: Value = serde_json::from_reader(GzDecoder::new(
        &include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/angstrom_ethereum_directory.json.gz"
        )[..],
    ))
    .unwrap();
    let addr = |v: &Value| v.as_str().unwrap().parse::<Address>().unwrap();
    let p = &a["protected"];
    let roles = p["constructor_roles"].as_array().unwrap();
    let payer = addr(&a["owner"]);
    let mut count = 0;
    for row in p["payments"].as_array().unwrap() {
        let k = &d["directory"][row["pool"].as_str().unwrap()]["key"];
        for leg in ["buy", "sell"].iter().filter_map(|n| row.get(*n)) {
            let x = &leg["exact"];
            let amount = x["spent"].as_str().unwrap().parse().unwrap();
            let net = x["received"].as_str().unwrap().parse().unwrap();
            let t = Terms {
                chain: 1,
                module: addr(&p["target"]),
                adapter: addr(&roles[0]),
                hook: addr(&roles[1]),
                manager: addr(&roles[2]),
                tokens: [addr(&k["currency0"]), addr(&k["currency1"])],
                fee: U24::from(1u32 << 23),
                spacing: I24::try_from(10).unwrap(),
                height: a["block"].as_u64().unwrap(),
                attestation: a["source_data"].as_str().unwrap().parse().unwrap(),
            };
            let input = addr(&leg["input"]);
            let output = addr(&leg["output"]);
            let proof = Proof {
                terms: t,
                amount,
                minimum: net,
                payer,
                input,
                output,
                deadline: U256::ONE,
            };
            let logs =
                serde_json::from_value::<Vec<crate::venues::bsc_dex::check::simulate::SimLog>>(
                    x["logs"].clone(),
                )
                .unwrap();
            assert_eq!(proof.measured(&logs), Some((amount, net)));
            let mut wrong = proof.clone();
            wrong.terms.height += 1;
            assert_eq!(wrong.measured(&logs), None);
            let mut wrong = proof.clone();
            wrong.amount += U256::ONE;
            assert_eq!(wrong.measured(&logs), None);
            let mut wrong = proof;
            wrong.terms.tokens.swap(0, 1);
            assert_eq!(wrong.measured(&logs), None);
            count += 1;
        }
    }
    assert_eq!(count, 12);
}
