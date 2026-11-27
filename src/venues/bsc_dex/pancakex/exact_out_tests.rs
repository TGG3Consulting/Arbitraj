use super::*;
use alloy_primitives::Bytes;
use flate2::read::GzDecoder;
use serde_json::Value;

#[test]
fn official_exact_outputs_keep_all_maximum_inputs_and_typed_digests() {
    let rows: Value = serde_json::from_reader(GzDecoder::new(
        &include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/pancakex_unsigned_exact_out.json.gz"
        )[..],
    ))
    .unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 4);
    for row in rows.as_array().unwrap() {
        let b = &row["body"];
        assert_eq!(b["type"], "EXACT_OUTPUT");
        let q = &row["response"]["message"]["bestOrder"]["order"];
        let info = &q["orderInfo"];
        let amount: U256 = info["input"]["endAmount"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let minimum = b["amount"].as_str().unwrap().parse().unwrap();
        let request = Request {
            owner: b["configs"][0]["swapper"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap(),
            input: b["tokenIn"].as_str().unwrap().parse().unwrap(),
            output: b["tokenOut"].as_str().unwrap().parse().unwrap(),
            amount,
            minimum,
            now: 0,
            exact_output: true,
        };
        let p = Roles {
            chain: b["tokenInChainId"].as_u64().unwrap(),
            reactor: info["reactor"].as_str().unwrap().parse().unwrap(),
            permit: q["permitData"]["domain"]["verifyingContract"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap(),
        };
        let bytes: Bytes = q["encodedOrder"].as_str().unwrap().parse().unwrap();
        let version = q["rfqVersion"].as_str().unwrap();
        let t = p.terms(version, &bytes, &request).unwrap();
        assert_eq!((t.maximum_input, t.min_output), (amount, minimum));
        assert!(t.quoted_input < t.maximum_input);
        assert_eq!(
            t.digest,
            row["digest"].as_str().unwrap().parse::<B256>().unwrap()
        );
        assert!(
            p.terms(
                version,
                &bytes,
                &Request {
                    amount: amount - U256::ONE,
                    ..request
                }
            )
            .is_none()
        );
        assert!(
            p.terms(
                version,
                &bytes,
                &Request {
                    minimum: minimum + U256::ONE,
                    ..request
                }
            )
            .is_none()
        );
    }
}
