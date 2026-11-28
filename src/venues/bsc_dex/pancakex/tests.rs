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
            exact_output: false,
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
                exact_output: false,
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
            super::cash_tests::paid(&terms, h);
            assert_eq!(h["receipt"]["blockHash"], h["header"]["hash"]);
            count += 1;
        }
    }
    assert_eq!(count, 3);
}

#[test]
fn ethereum_usdt_quotes_bind_original_assets_and_fill_through_the_pinned_reactor() {
    let body = read(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/pancakex_ethereum_usdt_quotes.json.gz"
    ));
    let archive: Value = serde_json::from_str(include_str!(
        "../../../../tests/data/first_run_dex_2026-11-26/pancakex_profiles.json"
    ))
    .unwrap();
    let profile_rows: Vec<String> =
        serde_json::from_value(archive["ethereum"]["contracts"].clone()).unwrap();
    let profile = profile::Profile::from_rows(1, &profile_rows).unwrap();
    let usdt: Address = "0xdac17f958d2ee523a2206206994597c13d831ec7"
        .parse()
        .unwrap();
    let listed = body["openPage"]["orders"].as_array().unwrap();
    assert_eq!(listed.len(), 2);
    assert!(listed.iter().all(|o| o["hasOrder"] == true
        && o["hasSignature"] == true
        && field::<Address>(&o["input"]["token"]) == usdt
        && o["input"]["startAmount"] == "0"
        && o["input"]["endAmount"] == "0"));
    let output_limits = [U256::from(59822095056064825u64), U256::from(166311624u64)];
    let rows = body["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    for (row, minimum) in rows.iter().zip(output_limits) {
        let b = &row["body"];
        assert_eq!(b["configs"][0]["useSyntheticQuotes"], false);
        let offer = client::Offer::read(&row["response"]).unwrap();
        let request = Request {
            owner: field(&b["configs"][0]["swapper"]),
            input: field(&b["tokenIn"]),
            output: field(&b["tokenOut"]),
            amount: field(&b["amount"]),
            minimum,
            exact_output: false,
            now: chrono::DateTime::parse_from_rfc2822(row["date"].as_str().unwrap())
                .unwrap()
                .timestamp() as u64,
        };
        let engine =
            field(&row["response"]["message"]["bestOrder"]["order"]["orderInfo"]["reactor"]);
        let quoted_roles = Roles {
            chain: 1,
            permit: profile.permit.address,
            reactor: engine,
        };
        let terms = quoted_roles
            .terms(&offer.version, &offer.encoded, &request)
            .unwrap();
        assert_eq!(terms.order_hash, offer.hash);
        assert_eq!(terms.min_output, minimum);
        if request.input == usdt {
            assert_eq!(profile.router.address, engine);
            assert!(offer.terms(&profile, &request).is_some());
        } else {
            assert_eq!(request.output, usdt);
            assert_ne!(profile.router.address, engine);
            let accepted = offer.terms(&profile, &request).unwrap();
            assert_eq!(accepted.roles.reactor, engine);
            assert_eq!(accepted.digest, terms.digest);
            let mut previous = profile.clone();
            previous.standalone_v1 = None;
            assert!(offer.terms(&previous, &request).is_none());
            let tokens = archive["ethereum"]["tokens"].as_array().unwrap();
            let tether = tokens
                .iter()
                .filter_map(|r| super::assets::AssetCode::parse(r.as_str()?, 1))
                .find(|v| v.address() == usdt)
                .unwrap();
            assert_eq!(tether.guaranteed_net(minimum), U256::from(165995632u64));
            assert_eq!(
                tether.guaranteed_net(U256::from(1_000_000_000_000u64)),
                U256::from(999951000000u64)
            );
        }
    }
}

#[test]
fn standalone_fill_recovers_the_original_eoa_and_the_two_canonical_wallet_changes() {
    let book = read(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/pancakex_v1_ethereum_net.json.gz"
    ));
    let archived = serde_json::from_slice::<Value>(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/pancakex_profiles.json"
    ))
    .unwrap();
    let profile = profile::Profile::from_rows(
        1,
        &serde_json::from_value::<Vec<String>>(archived["ethereum"]["contracts"].clone()).unwrap(),
    )
    .unwrap();
    let token = archived["ethereum"]["tokens"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| assets::AssetCode::parse(r.as_str()?, 1))
        .find(|r| matches!(r, assets::AssetCode::TetherNet(_)))
        .unwrap();
    let rows = book["payments"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    for row in rows {
        let o = &row["order"];
        let req = Request {
            owner: field(&o["offerer"]),
            input: field(&o["input"]["token"]),
            output: token.address(),
            amount: field(&row["walletDebit"]),
            minimum: U256::ONE,
            exact_output: false,
            now: field::<U256>(&row["header"]["timestamp"]).to(),
        };
        let offer = client::Offer {
            filler: field(&o["filler"]),
            id: "recorded".into(),
            version: "v1".into(),
            encoded: field(&o["encodeOrder"]),
            hash: field(&o["hash"]),
        };
        let terms = offer.terms(&profile, &req).unwrap();
        assert_eq!(terms.roles.reactor, profile.standalone_v1.unwrap().address);
        assert_eq!(
            field::<Signature>(&o["signature"])
                .recover_address_from_prehash(&terms.digest)
                .unwrap(),
            req.owner
        );
        let logs =
            serde_json::from_value::<Vec<alloy::rpc::types::Log>>(row["receipt"]["logs"].clone())
                .unwrap();
        let cash = terms.cash_net(&logs, &[], false, &token).unwrap();
        let val = |state: &str, key: &str| field::<U256>(&row[state][key]["result"]);
        assert_eq!(
            cash.received,
            val("after", "outputBalance") - val("before", "outputBalance")
        );
        assert_eq!(
            cash.paid,
            val("before", "inputBalance") - val("after", "inputBalance")
        );
        assert_eq!(cash.received, field::<U256>(&row["walletCredit"]));
        assert_eq!(row["receipt"]["blockHash"], row["header"]["hash"]);
        assert_eq!(row["receipt"]["transactionHash"], o["transactionHash"]);
        let bit = (terms.nonce & U256::from(255)).to::<usize>();
        assert!(!val("before", "nonceBitmap").bit(bit));
        assert!(val("after", "nonceBitmap").bit(bit));
        assert!(terms.cash_net(&[], &[], false, &token).is_none());
        assert!(terms.cash_net(&logs, &[], true, &token).is_none());
    }
}
