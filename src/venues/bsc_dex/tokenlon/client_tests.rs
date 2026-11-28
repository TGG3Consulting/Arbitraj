//! Записанные ответы издателя PMMV5 на Ethereum: котировка и отказ последней заявки.
//! Поля, которые архив не сохранял, в тесте сообщения указываются отдельно.
use super::*;
use alloy::primitives::Signature;
use serde_json::json;
use std::str::FromStr;

fn capture() -> Value {
    serde_json::from_str(include_str!(
        "../../../../tests/data/first_run_dex_2026-11-26/tokenlon_ethereum.json"
    ))
    .unwrap()
}

#[test]
fn ethereum_wallet_auth_signs_only_the_sdk_timestamp() {
    let key = PrivateKeySigner::random();
    let timestamp = 1_795_778_236;
    let msg = auth::signed(timestamp, &key).unwrap();
    assert_eq!(msg["method"], "auth.getSdkJwtToken");
    assert_eq!(msg["params"]["timestamp"], timestamp);
    let sig: Signature = msg["params"]["signature"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        sig.recover_address_from_msg(timestamp.to_string().as_bytes())
            .unwrap(),
        key.address()
    );
    assert!(auth::allowed("https://publisher.tokenlon.im/rpc"));
    let url = auth::socket_url("opaque&redirect=https://example.com").unwrap();
    assert_eq!(url.host_str(), Some("publisher.tokenlon.im"));
    assert_eq!(url.scheme(), "wss");
    assert!(url.path().starts_with("/exchange/") && url.path().ends_with("/websocket"));
    let mut query = url.query_pairs();
    let (name, value) = query.next().unwrap();
    assert_eq!(name, "Authorization");
    assert_eq!(value, "Token opaque&redirect=https://example.com");
    assert!(query.next().is_none());
    assert!(auth::socket_url("bad\nheader").is_err());
    for invalid in [
        "http://publisher.tokenlon.im/rpc",
        "https://publisher.tokenlon.im/rpc?target=x",
        "https://publisher.tokenlon.im@evil.example/rpc",
        "https://publisher.tokenlon.im/another",
    ] {
        assert!(!auth::allowed(invalid));
    }
}
#[test]
fn rate_order_and_final_decline_follow_separate_subscriptions() {
    let cap = capture();
    let observed = &cap["observed_taker"];
    let q0 = &observed["ws-probe-mm"]["responses"]["q0"];
    let address = |k: &str| q0[k].as_str().unwrap().parse().unwrap();
    let q = Query {
        base: "USDC".into(),
        quote: "USDT".into(),
        side: Side::Sell,
        amount: Decimal::from(1000),
        base_dec: 6,
        input: address("takerAssetAddress"),
        output: address("makerAssetAddress"),
    };
    let owner: Address = observed["ws-probe-mm"]["wallet"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        q.topic(Stage::Rate, owner).unwrap(),
        "/user/rate/USDC_USDT/SELL/1000"
    );
    assert_eq!(
        q.topic(Stage::LastOrder, owner).unwrap(),
        format!(
            "/user/lastOrder/USDC_USDT/SELL/1000/{}",
            owner.to_checksum(None)
        )
    );
    let at = observed["ws-probe-mm"]["utc_unix"].as_u64().unwrap();
    let mut order = q0.clone();
    // The recording stored selected financial fields; supply explicit owner and expiry for this wire-shape test.
    order["feeRecipientAddress"] = json!(owner);
    order["expirationTimeSeconds"] = json!(at + 1);
    let preview = json!({"exchangeable":true,"order":order});
    let mut other = preview.clone();
    other["order"]["takerAssetAmount"] = json!("999999999");
    assert_eq!(
        pmm_order(other, owner, &q, at, false),
        Err(Failure::NoOffer)
    );
    assert!(pmm_order(preview.clone(), owner, &q, at, false).is_ok());
    assert_eq!(
        pmm_order(preview.clone(), owner, &q, at, true),
        Err(Failure::NoOffer)
    );
    assert_eq!(
        pmm_order(preview.clone(), Address::ZERO, &q, at, false),
        Err(Failure::NoOffer)
    );
    assert_eq!(
        pmm_order(preview, owner, &q, at + 1, false),
        Err(Failure::NoOffer)
    );
    for s in observed["sdk_flow"]["samples"].as_array().unwrap() {
        let last = &s["result"][1];
        let packet = format!(
            "MESSAGE\nsubscription:q2\n\n{}\0",
            json!({"exchangeable":last["ok"],"reason":last["reason"]})
        );
        assert_eq!(socket::publication(&packet, "q1"), Ok(None));
        let v = socket::publication(&packet, "q2").unwrap().unwrap();
        assert_eq!(pmm_order(v, owner, &q, at, true), Err(Failure::Balance));
    }
    let rate = &observed["ws-probe"]["responses"]["q0"];
    assert_eq!(
        rate_value(rate, Decimal::ONE),
        Ok(Decimal::from_str("1724.3331434598999294").unwrap())
    );
    assert_eq!(rate_value(rate, Decimal::from(3000)), Err(Failure::NoOffer));
    let mut fake = q;
    fake.base = "USDC\ndestination:other".into();
    assert!(fake.topic(Stage::Order, owner).is_none());
}
