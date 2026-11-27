use super::*;
use flate2::read::GzDecoder;
use serde_json::Value;

#[test]
fn included_module_keeps_the_actual_payout_separate_from_the_pool_swap_gross() {
    let f: Value = serde_json::from_reader(GzDecoder::new(
        &include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/shibaswap_v2_ethereum_payments.json.gz"
        )[..],
    ))
    .unwrap();
    let mut count = 0;
    for row in f["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["wire"] == "module")
    {
        let parsed = |p: &Value| p.as_str().unwrap().parse::<Address>().unwrap();
        let signed = &row["signed"];
        let data: Bytes = signed["data"].as_str().unwrap().parse().unwrap();
        let input = parsed(&row["source"]);
        let output = parsed(&row["target"]);
        let payer = parsed(&signed["from"]);
        let qty = row["spent"].as_str().unwrap().parse().unwrap();
        let net = row["received"].as_str().unwrap().parse().unwrap();
        let logs: Vec<Log> = serde_json::from_value(row["logs"].clone()).unwrap();
        let mut p = Proof {
            pool: parsed(&row["pool"]),
            module: parsed(&signed["to"]),
            payer,
            input,
            output,
            amount: qty,
            minimum: U256::from_be_slice(&data[4 + 4 * 32..4 + 5 * 32]),
            deadline: U256::from_be_slice(&data[data.len() - 32..]),
            module_hash: f["module_hash"].as_str().unwrap().parse().unwrap(),
            exact_codes: None,
            verified: None,
        };
        assert_eq!(p.data(), data);
        assert_eq!(p.exact(&logs), Some(net));
        assert_eq!(p.amounts(&logs, payer, input, output), None);
        p.verified = Some(net);
        assert_eq!(p.amounts(&logs, payer, input, output), Some((qty, net)));
        // При том же валовом Swap корневой результат может оказаться меньше из-за удержания.
        p.minimum = net - U256::ONE;
        p.verified = Some(p.minimum);
        assert_eq!(
            p.amounts(&logs, payer, input, output),
            Some((qty, p.minimum))
        );
        let restored: Proof = serde_json::from_value(serde_json::to_value(&p).unwrap()).unwrap();
        assert_eq!(restored.verified, None);
        count += 1;
    }
    assert_eq!(count, 24);
}
