//! Нелинейная цена при фактическом входе AMM и округление комиссии по подписанным данным трёх реальных операций.
use super::{relay_tests::limits, wire::executeSwapIntentsCall};
use alloy::{
    primitives::{Address, Bytes, U256},
    sol_types::SolCall,
};
use axum::{Json, Router, extract::State, routing::post};
use serde_json::{Value, json};
use std::sync::Arc;

#[test]
fn three_historical_fees_match_the_scheduled_usd_and_price_supplied_to_the_router() {
    let raw = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/ryze_base_historical_receipts.json.gz"
    );
    let history: Vec<Value> =
        serde_json::from_reader(flate2::read::GzDecoder::new(&raw[..])).unwrap();
    let sources: Value = serde_json::from_str(include_str!(
        "../../../../tests/data/first_run_dex_2026-11-26/ryze_base_tokens.json"
    ))
    .unwrap();
    let fees = sources["historical_intent_fees"].as_array().unwrap();
    for ((row, fee), (decimals, charged)) in
        history
            .iter()
            .zip(fees)
            .zip([(6, 10_000u64), (18, 5_944_578_692_847), (8, 17)])
    {
        assert_eq!(row["transactionHash"], fee["transaction"]);
        let usd = U256::from_str_radix(fee["usd_wad"].as_str().unwrap(), 10).unwrap();
        let input: Bytes = row["transaction"]["input"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let call = executeSwapIntentsCall::abi_decode_validate(&input).unwrap();
        let o = &call.signedIntents[0].intent;
        let price = call
            .cexPriceData
            .iter()
            .find(|p| p.token == o.tokenIn)
            .unwrap();
        assert_eq!(
            super::price::input_fee(usd, price.priceInWad, decimals, o.path.len()),
            Some(U256::from(charged))
        );
    }
}
#[tokio::test]
async fn nonlinear_pool_is_requoted_at_net_input_after_rounding_the_flat_fee_per_hop() {
    use super::price::{Market, input_fee};
    assert_eq!(
        input_fee(U256::from(3), U256::from(2), 0, 2),
        Some(U256::from(4))
    );
    assert_eq!(input_fee(U256::ONE, U256::ZERO, 18, 1), None);
    let (a, b, p, owner) = (
        Address::repeat_byte(1),
        Address::repeat_byte(2),
        Address::repeat_byte(3),
        Address::repeat_byte(4),
    );
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let observed = calls.clone();
    let mock=Router::new().route("/quote",post(move|State(seen):State<Arc<std::sync::Mutex<Vec<u64>>>>,Json(req):Json<Value>|async move{
        assert_eq!(req["userAddress"],json!(owner));
        let n=req["amountIn"].as_str().unwrap().parse::<u64>().unwrap();seen.lock().unwrap().push(n);
        let output=n*1000/(100+n);
        Json(json!({"input":{"token":a,"decimals":0,"amount":n.to_string()},"output":{"token":b,"decimals":0,"amount":output.to_string()},
            "path":[a,b],"steps":[{"pool":p,"tokenIn":a,"tokenOut":b,"amountIn":n.to_string(),"amountOut":output.to_string()}],
            "prices":[{"token":a,"blendedPrice":"1"}],"timestamp":chrono::Utc::now()}))
    })).with_state(observed);
    let socket = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let addr = socket.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(socket, mock).await.unwrap() });
    let pools = [(p, a, b)];
    let m = Market {
        wallet: owner,
        input: a,
        output: b,
        decimals: (0, 0),
        pools: &pools,
        slippage_bps: 20,
        max_age_ms: 5000,
    };
    let q = m
        .measure(
            format!("http://{addr}/quote").parse().unwrap(),
            limits(),
            U256::from(10),
            U256::from(100),
            false,
        )
        .await
        .unwrap();
    assert_eq!(calls.lock().unwrap().as_slice(), [100, 90]);
    assert_eq!(
        (q.gross, q.net, q.expected, q.fee),
        (
            U256::from(100),
            U256::from(90),
            U256::from(473),
            U256::from(10)
        )
    );
    task.abort();
}
