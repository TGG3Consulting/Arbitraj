//! Наблюдавшийся отказ /prices, публичная книга /pairs и схема успеха Dexalot Simple Swap §3b.
use super::*;
use crate::venues::bsc_dex::dexalot::channel::{ChannelPrices, PRICES};
use reqwest::StatusCode;

fn observation() -> Value {
    serde_json::from_str(include_str!(
        "../../../../tests/data/first_run_dex_2026-11-26/dexalot_prices_access.json"
    ))
    .unwrap()
}

fn public_book() -> Value {
    serde_json::from_str(include_str!(
        "../../../../tests/data/first_run_dex_2026-11-26/dexalot_base_indicative.json"
    ))
    .unwrap()
}

#[test]
fn manual_check_builds_the_recorded_base_prices_request_and_requires_a_key() {
    let observation = observation();
    let mut url = Url::parse(public_book()["url"].as_str().unwrap()).unwrap();
    assert_eq!(key_allowed(&url, true, true), Ok(true));
    assert!(key_allowed(&url, true, false).is_err());
    let chain = crate::settings::evm_profiles::profile("base_dex")
        .unwrap()
        .evm_chain_id;
    append_path(&mut url, PRICES, &[("chainid", chain.to_string())]).unwrap();
    assert_eq!(url.as_str(), observation["url"].as_str().unwrap());

    // Ни URL документации, ни URL полного запроса не могут стать корнем, куда отправляется ключ.
    let docs = Url::parse(
        observation["documented_example"]["source"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    for other in [url, docs] {
        assert!(key_allowed(&other, true, true).is_err());
        assert_eq!(key_allowed(&other, false, true), Ok(false));
    }
}

#[test]
fn recorded_key_refusal_exposes_only_status_and_known_reason_code() {
    let observation = observation();
    let code = StatusCode::from_u16(observation["status"].as_u64().unwrap() as u16).unwrap();
    let body = observation["body"].to_string();
    let Err(denied) = decode::<ChannelPrices>(code, &body) else {
        panic!("Dexalot отказал без ключа");
    };
    assert_eq!(denied, "Dexalot: HTTP 400, FQR-001, ключ канала не принят");
    assert!(decode::<ChannelPrices>(StatusCode::OK, &body).is_err());
}

#[test]
fn published_prices_schema_is_distinct_from_a_successful_public_pairs_request() {
    let observed = observation();
    let success = observed["documented_example"]["body"].to_string();
    assert!(decode::<ChannelPrices>(StatusCode::OK, &success).is_ok());
    assert!(decode::<ChannelPrices>(StatusCode::OK, &public_book()["pairs"].to_string()).is_err());
    assert!(decode::<ChannelPrices>(StatusCode::TEMPORARY_REDIRECT, &success).is_err());
}
