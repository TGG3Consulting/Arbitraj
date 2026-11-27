use super::*;
use alloy::primitives::Signature;
use alloy_primitives::Bytes;
use flate2::read::GzDecoder;
use serde_json::Value;
use std::str::FromStr;

fn read(raw: &[u8]) -> Value {
    serde_json::from_reader(GzDecoder::new(raw)).unwrap()
}
fn field<T: FromStr>(v: &Value) -> T
where
    T::Err: std::fmt::Debug,
{
    v.as_str().unwrap().parse().unwrap()
}
#[test]
fn four_unsigned_orders_bind_both_assets_local_minimum_account_and_permit_domain() {
    let book = read(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/pancakex_unsigned_quotes.json.gz"
    ));
    let mut ok = 0;
    let mut declined = 0;
    for row in book.as_array().unwrap() {
        let b = &row["body"];
        assert_eq!(b["configs"][0]["useSyntheticQuotes"], false);
        let msg = &row["response"]["message"];
        if row["status"] == 400 {
            assert_eq!(b["tokenInChainId"], 8453);
            assert!(
                msg["error"]
                    .as_str()
                    .unwrap()
                    .contains("must include a RWA")
            );
            declined += 1;
            continue;
        }
        let q = &msg["bestOrder"]["order"];
        let info = &q["orderInfo"];
        let d = &q["permitData"]["domain"];
        let roles = Roles {
            chain: b["tokenInChainId"].as_u64().unwrap(),
            reactor: field(&info["reactor"]),
            permit: field(&d["verifyingContract"]),
        };
        let req = Request {
            owner: field(&b["configs"][0]["swapper"]),
            input: field(&b["tokenIn"]),
            output: field(&b["tokenOut"]),
            amount: field(&b["amount"]),
            minimum: field(&info["outputs"][0]["endAmount"]),
            now: 0,
        };
        let raw: Bytes = field(&q["encodedOrder"]);
        let v = q["rfqVersion"].as_str().unwrap();
        let terms = roles.terms(v, &raw, &req).unwrap();
        assert_eq!(terms.order_hash, field::<B256>(&q["orderHash"]));
        assert_eq!(terms.min_output, req.minimum);
        assert_eq!(
            terms.quoted_output,
            field::<U256>(&info["outputs"][0]["startAmount"])
        );
        assert!(
            roles
                .terms(
                    v,
                    &raw,
                    &Request {
                        minimum: req.minimum + U256::ONE,
                        ..req
                    }
                )
                .is_none()
        );
        assert!(
            roles
                .terms(
                    v,
                    &raw,
                    &Request {
                        owner: roles.reactor,
                        ..req
                    }
                )
                .is_none()
        );
        assert!(
            roles
                .terms(
                    v,
                    &raw,
                    &Request {
                        now: terms.deadline + 1,
                        ..req
                    }
                )
                .is_none()
        );
        assert!(
            roles
                .terms(
                    v,
                    &raw,
                    &Request {
                        amount: req.amount + U256::ONE,
                        ..req
                    }
                )
                .is_none()
        );
        let wrong = Roles {
            chain: roles.chain + 1,
            ..roles
        }
        .terms(v, &raw, &req)
        .unwrap();
        assert_ne!(terms.digest, wrong.digest);
        ok += 1;
    }
    assert_eq!((ok, declined), (4, 2));
}
#[test]
fn three_mined_signatures_match_original_owners_and_cash() {
    let books: &[&[u8]] = &[
        include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/pancakex_ethereum_history.json.gz"
        ),
        include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/pancakex_base_history.json.gz"
        ),
    ];
    let mut count = 0;
    for raw in books {
        let book = read(raw);
        let permit = book["roles"]
            .as_object()
            .unwrap()
            .values()
            .find_map(|r| r["facets_and_permit"]["permit2()"]["address"].as_str())
            .unwrap()
            .parse()
            .unwrap();
        for h in book["history"].as_array().unwrap() {
            let o = &h["order"];
            let roles = Roles {
                chain: o["chainId"].as_u64().unwrap(),
                reactor: field(&o["reactor"]),
                permit,
            };
            let req = Request {
                owner: field(&o["offerer"]),
                input: field(&o["input"]["token"]),
                output: field(&o["outputs"][0]["token"]),
                amount: field(&h["paid"]["amount"]),
                minimum: U256::ONE,
                now: u64::from_str_radix(
                    h["header"]["timestamp"]
                        .as_str()
                        .unwrap()
                        .trim_start_matches("0x"),
                    16,
                )
                .unwrap(),
            };
            let version = match o["type"].as_str().unwrap() {
                "EXCLUSIVE_DUTCH_ORDER" => "v1",
                "EXCLUSIVE_DUTCH_ORDER_V3" => "v3",
                _ => unreachable!(),
            };
            let terms = roles
                .terms(version, &field::<Bytes>(&o["encodeOrder"]), &req)
                .unwrap();
            assert_eq!(terms.order_hash, field::<B256>(&o["hash"]));
            let sig: Signature = field(&o["signature"]);
            assert_eq!(
                sig.recover_address_from_prehash(&terms.digest).unwrap(),
                req.owner
            );
            assert!(field::<U256>(&h["received"]["amount"]) >= terms.min_output);
            assert_eq!(h["receipt"]["blockHash"], h["header"]["hash"]);
            count += 1;
        }
    }
    assert_eq!(count, 3);
}
