//! Источник digest и кассовых дельт — восемь канонических исполненных сделок.
use super::{
    api::instruments,
    wire::{Offer, Quote},
};
use alloy::signers::{SignerSync, local::PrivateKeySigner};
use alloy_primitives::{Address, B256, U256};
use serde_json::{Value, json};
use std::io::Read;

#[test]
fn historical_digest_and_deadline_match_eight_payments() {
    let mut raw = Vec::new();
    flate2::read::GzDecoder::new(
        include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/clipper_rfq_access.json.gz"
        )
        .as_slice(),
    )
    .read_to_end(&mut raw)
    .unwrap();
    let capture: Value = serde_json::from_slice(&raw).unwrap();
    let mut count = 0;
    for net in capture["networks"].as_object().unwrap().values() {
        for r in net["original"].as_array().unwrap() {
            let a = |k: &str| r[k].as_str().unwrap().parse::<Address>().unwrap();
            let n = |r: &Value| U256::from_str_radix(r.as_str().unwrap(), 10).unwrap();
            let o = Offer {
                chain: r["chain"].as_u64().unwrap(),
                pool: a("pool"),
                owner: a("recipient"),
                input: a("input"),
                output: a("output"),
                amount: n(&r["quoted"][0]),
                received: n(&r["quoted"][1]),
                until: n(&r["packed_good_until"]),
            };
            assert_eq!(
                o.digest(),
                r["digest"].as_str().unwrap().parse::<B256>().unwrap()
            );
            assert_eq!(Some(o.expires()), r["expires"].as_u64());
            assert!(o.expires() >= r["timestamp"].as_u64().unwrap());
            assert!(n(&r["delivered"][0]) > U256::ZERO && n(&r["delivered"][1]) > U256::ZERO);
            let key = PrivateKeySigner::random();
            let sig = key.sign_hash_sync(&o.digest()).unwrap();
            let mut q:Quote=serde_json::from_value(json!({"chain_id":o.chain,"clipper_exchange_address":o.pool,"input_asset_address":o.input,
            "output_asset_address":o.output,"input_amount":o.amount.to_string(),"output_amount":o.received.to_string(),"good_until":o.until.to_string(),
            "destination_address":o.owner,"signature":{"v":27+u8::from(sig.v()),"r":B256::from(sig.r()),"s":B256::from(sig.s())}})).unwrap();
            assert!(o.valid(&q, key.address()));
            assert!(!o.valid(&q, a("signer")));
            q.destination_address = Address::ZERO;
            assert!(!o.valid(&q, key.address()));
            q.destination_address = o.owner;
            q.chain_id += 1;
            assert!(!o.valid(&q, key.address()));
            count += 1;
        }
    }
    assert_eq!(count, 8);
}
#[test]
fn api_symbols_belong_to_one_exchange_on_the_requested_chain() {
    let pool = Address::repeat_byte(1);
    let a = Address::repeat_byte(2);
    let b = Address::repeat_byte(3);
    let mut book = json!({"pool_type":"onchain","pools":[{"pool":{"chain_id":8453,"address":pool,"swaps_enabled":true},
              "assets":[{"address":a,"name":"ETH"},{"address":b,"name":"USDC"}]}]});
    assert_eq!(
        instruments(&book, 8453, pool, [a, b]),
        Some(["ETH".into(), "USDC".into()])
    );
    assert!(instruments(&book, 1, pool, [a, b]).is_none());
    book["pools"][0]["assets"][1]["name"] = json!("ETH");
    assert!(instruments(&book, 8453, pool, [a, b]).is_none());
}

/// Истёкший опубликованный пример GET /rfq/v2/quote-sign из https://docs.sushi.com/api/blade/swagger.
#[test]
fn published_sushi_offer_recovers_the_canonical_designated_signer() {
    let q:Quote=serde_json::from_str(r#"{"chain_id":1,"clipper_exchange_address":"0x655eDCE464CC797526600a462A8154650EEe4B77","input_asset_address":"0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2","output_asset_address":"0x6B175474E89094C44Da98b954EedeAC495271d0F","input_amount":"780000000000000","output_amount":"3504443835292476928","good_until":"451117153933605942738249439739673213871765011081718076541610938493960","destination_address":"0x960376b3F62f41E7e66809a05D1C5afdFD60A0E9","signature":{"v":28,"r":"0xb996d67a0f80da5322e0fba14afd82e63e51cfc9ed09cc3377e84c7dfd05f9a1","s":"0x378c0baaa171606042171fd8886ac007017854f98aa0a85b5ae0c9ce892f1aae"}}"#).unwrap();
    let mut o = Offer {
        chain: q.chain_id,
        pool: q.clipper_exchange_address,
        owner: q.destination_address,
        input: q.input_asset_address,
        output: q.output_asset_address,
        amount: q.input_amount.parse().unwrap(),
        received: q.output_amount.parse().unwrap(),
        until: q.good_until.parse().unwrap(),
    };
    let signer = "0x08938a61ba9523298dbcacee0cda5b371fb7f1f8"
        .parse()
        .unwrap();
    assert_eq!(o.expires(), 1_755_212_808);
    assert!(o.valid(&q, signer));
    o.amount += U256::ONE;
    assert!(!o.valid(&q, signer));
}

#[test]
fn blade_key_is_scoped_to_its_issuer_even_when_the_api_path_is_configurable() {
    let key = r#"{"origin":"https://blade-api.sushi.com","key":"assigned"}"#;
    assert!(super::api::scoped_key("https://blade-api.sushi.com/partner/rfq/v2", key).is_some());
    assert!(super::api::scoped_key("https://blade-api.sushi.com/partner/rfq/v2", "unbound").is_none());
    for url in [
        "http://blade-api.sushi.com/rfq", "https://blade-api.sushi.com:8443/rfq",
        "https://blade-api.sushi.com.example.org/rfq", "https://example.org/",
        "https://user@blade-api.sushi.com/rfq", "https://blade-api.sushi.com/rfq?token=x",
        "https://blade-api.sushi.com/rfq#fragment",
    ] {
        assert!(super::api::scoped_key(url, key).is_none());
    }
}

#[test]
fn rotating_issuer_changes_the_destination_only_with_its_credential() {
    let stored = r#"{"origin":"https://partner.example.org:9443","key":"assigned"}"#;
    let permitted = "https://partner.example.org:9443/quotes";
    assert_eq!(super::api::scoped_key(permitted, stored).unwrap().1, "assigned");
    assert!(super::api::scoped_key("https://partner.example.org/quotes", stored).is_none());
    assert!(super::api::scoped_key("https://blade-api.sushi.com/rfq/v2", stored).is_none());
    assert!(super::api::scoped_key(permitted, r#"{"origin":"https://partner.example.org:9443"}"#).is_none());
}
