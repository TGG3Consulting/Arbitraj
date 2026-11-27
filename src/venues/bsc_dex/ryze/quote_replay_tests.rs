//! Записи публичного Router: измеряем заявку на полное списание с исходным запросом и проверяем выход в атомах.
use super::{price::{Market, Quoted}, relay_tests::limits};
use alloy::primitives::{Address, U256};
use axum::{Json, Router, routing::post};
use serde_json::Value;
use std::{collections::VecDeque, sync::{Arc, Mutex}};

fn archive() -> Value {
    serde_json::from_str(include_str!(
        "../../../../tests/data/first_run_dex_2026-11-26/ryze_service_responses.json"
    )).unwrap()
}

async fn replay(source: &Value, calls: &[Value], buy: bool, buy_cap: U256, requests: u64) -> Option<Quoted> {
    let req = &calls[0]["request"];
    let body = &calls[0]["body"];
    let address = |v: &Value| v.as_str().unwrap().parse::<Address>().unwrap();
    let pools: Vec<_> = source["settings_pools"].as_array().unwrap().iter()
        .map(|v| (address(&v[0]), address(&v[1]), address(&v[2]))).collect();
    let parts = super::router_budget_tests::parts("ryze-quote-replay", address(&req["userAddress"]), requests).await;
    let market = Market {
        ctx: &parts.ctx,
        wallet: address(&req["userAddress"]),
        input: address(&req["tokenIn"]),
        output: address(&req["tokenOut"]),
        decimals: (body["input"]["decimals"].as_u64().unwrap() as u8,
                   body["output"]["decimals"].as_u64().unwrap() as u8),
        pools: &pools,
        buy_cap,
        slippage_bps: req["slippageTolerance"].as_u64().unwrap() as u16,
        max_age_ms: u64::MAX,
    };
    let remaining = Arc::new(Mutex::new(VecDeque::from(calls.to_vec())));
    let queue = remaining.clone();
    let endpoint = Router::new().route("/quote", post(move |Json(actual): Json<Value>| {
        let row = queue.lock().unwrap().pop_front().unwrap();
        async move {
            assert_eq!(actual, row["request"]);
            Json(row["body"].clone())
        }
    }));
    let socket = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = socket.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(socket, endpoint).await.unwrap() });
    let qty = req[if buy { "amountOut" } else { "amountIn" }].as_str().unwrap().parse().unwrap();
    let fee = source["fee_reference"]["usd_wad"].as_str().unwrap().parse().unwrap();
    let measured = market.measure(format!("http://{addr}/quote").parse().unwrap(), limits(), fee, qty, buy)
        .await;
    assert!(remaining.lock().unwrap().is_empty());
    server.abort();
    measured
}

#[tokio::test]
async fn recorded_sell_uses_whole_wallet_debit_once() {
    let source = archive();
    let sale = source["router_quotes"].as_array().unwrap().iter().find(|r| r["buy"].as_bool() == Some(false)).unwrap();
    let quote = replay(&source, &sale["calls"].as_array().unwrap()[..1], false, U256::MAX, 100).await.unwrap();
    assert_eq!((quote.gross, quote.net, quote.expected), (
        U256::from(1_000_000_000_000_000u64), U256::from(994_021_230_610_103u64), U256::from(1_660_790)
    ));
}

#[tokio::test]
async fn measured_buy_rejects_older_shortfall_and_finds_later_funded_size() {
    let source = archive();
    let original = source["router_quotes"].as_array().unwrap().iter().find(|r| r["buy"].as_bool() == Some(true)).unwrap();
    let old = replay(&source, std::slice::from_ref(&original["gross_input_control"]), false, U256::MAX, 100).await.unwrap();
    let target = original["calls"][0]["request"]["amountOut"].as_str().unwrap().parse::<U256>().unwrap();
    assert_eq!((old.gross, old.expected), (U256::from(1_684_341), U256::from(999_844_560_250_825u64)));
    assert!(!old.covers(target));

    let newer: Value = serde_json::from_str(include_str!(
        "../../../../tests/data/first_run_dex_2026-11-26/ryze_buy_gross_control.json"
    )).unwrap();
    let calls = newer["calls"].as_array().unwrap();
    // Первые три чтения достигают исходного потолка в 1,668727 USDC с недобором.
    assert!(replay(&source, &calls[..3], true, U256::from(1_668_727), 100).await.is_none());
    // Четвёртый запрос нужен для покупки, но у общего окна осталось ровно два запроса.
    assert!(replay(&source, &calls[..2], true, U256::MAX, 2).await.is_none());
    let final_quote = replay(&source, calls, true, U256::MAX, 100).await.unwrap();
    assert_eq!((final_quote.gross, final_quote.net, final_quote.expected), (
        U256::from(1_673_732), U256::from(1_663_732), U256::from(1_002_996_653_943_068u64)
    ));
    assert!(final_quote.covers(target));
}
