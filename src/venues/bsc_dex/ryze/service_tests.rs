//! О-35: архив ответов Router и Relayer, разный смысл 429 отправки и статуса, восстановление без новой подписи.
use super::{
    budget, http, ledger::Marker, price::Market, relay_tests::limits, wire::executeSwapIntentsCall,
};
use crate::{
    core::types::{Asset, DexRoute, RouteMode, Side, TradeRequest, TradeStatus},
    gateway::settlement_intent as intent,
    settings::scope::Scope,
    venues::bsc_dex::hashflow::recovery_tests::{directory, parts},
};
use alloy::{
    primitives::{Address, B256, Bytes, U256},
    sol_types::SolCall,
};
use axum::{
    Json, Router,
    http::StatusCode,
    routing::{get, post},
};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
fn source() -> Value {
    serde_json::from_str(include_str!(
        "../../../../tests/data/first_run_dex_2026-11-26/ryze_service_responses.json"
    ))
    .unwrap()
}

#[tokio::test]
async fn router_book_stops_when_recorded_input_or_buy_minimum_disagrees() {
    let data = source();
    for example in data["router_quotes"].as_array().unwrap() {
        let calls = example["calls"].as_array().unwrap();
        let seen = Arc::new(Mutex::new(VecDeque::from(calls.clone())));
        let seq = seen.clone();
        let req = &calls[0]["request"];
        let body = &calls[0]["body"];
        let address = |v: &Value| v.as_str().unwrap().parse::<Address>().unwrap();
        let a = address(&req["tokenIn"]);
        let b = address(&req["tokenOut"]);
        let pools: Vec<_> = data["settings_pools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| (address(&v[0]), address(&v[1]), address(&v[2])))
            .collect();
        let m = Market {
            wallet: address(&req["userAddress"]),
            input: a,
            output: b,
            decimals: (
                body["input"]["decimals"].as_u64().unwrap() as u8,
                body["output"]["decimals"].as_u64().unwrap() as u8,
            ),
            pools: &pools,
            slippage_bps: req["slippageTolerance"].as_u64().unwrap() as u16,
            max_age_ms: u64::MAX,
        };
        let endpoint = Router::new().route(
            "/quote",
            post(move |Json(got): Json<Value>| {
                let r = seq.lock().unwrap().pop_front().unwrap();
                async move {
                    assert_eq!(got, r["request"]);
                    Json(r["body"].clone())
                }
            }),
        );
        let sock = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let addr = sock.local_addr().unwrap();
        let run = tokio::spawn(async move { axum::serve(sock, endpoint).await.unwrap() });
        let buy = example["buy"].as_bool().unwrap();
        let qty = req[if buy { "amountOut" } else { "amountIn" }]
            .as_str()
            .unwrap()
            .parse::<U256>()
            .unwrap();
        let fee = data["fee_reference"]["usd_wad"]
            .as_str()
            .unwrap()
            .parse::<U256>()
            .unwrap();
        let measured = m
            .measure(
                format!("http://{addr}/quote").parse().unwrap(),
                limits(),
                fee,
                qty,
                buy,
            )
            .await;
        // У SELL уже первый steps[0].amountIn меньше input.amount. Для BUY второй exact-input
        // ответ даёт меньше подписываемого минимального выхода. В обоих случаях ордера нет.
        assert!(measured.is_none());
        let sent = calls.len() - seen.lock().unwrap().len();
        assert_eq!(sent, if buy { 2 } else { 1 });
        run.abort();
    }
}

#[tokio::test]
async fn post_429_survives_restart_while_get_429_keeps_original_digest_unresolved() {
    let data = source();
    let real = data["relayer_status"]["body"].clone();
    let digest = real["intent"]["intentId"]
        .as_str()
        .unwrap()
        .parse::<B256>()
        .unwrap();
    let tx = real["intent"]["txHash"]
        .as_str()
        .unwrap()
        .parse::<B256>()
        .unwrap();
    let server = Router::new()
        .route(
            "/status",
            get(move || {
                let r = real.clone();
                async move { Json(r) }
            }),
        )
        .route(
            "/limited",
            post(|| async {
                (
                    StatusCode::TOO_MANY_REQUESTS,
                    [("retry-after", "7")],
                    vec![0xff],
                )
            })
            .get(|| async {
                (
                    StatusCode::TOO_MANY_REQUESTS,
                    [("retry-after", "7")],
                    vec![0xff],
                )
            }),
        );
    let sock = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let addr = sock.local_addr().unwrap();
    let run = tokio::spawn(async move { axum::serve(sock, server).await.unwrap() });
    let url = |suffix: &str| format!("http://{addr}/{suffix}").parse().unwrap();
    assert!(
        matches!(http::transaction(url("status"),digest,limits()).await,http::Status::Hint(Some(id)) if id==tx)
    );

    let archive = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/ryze_base_historical_receipts.json.gz"
    );
    let rows: Vec<Value> =
        serde_json::from_reader(flate2::read::GzDecoder::new(archive.as_slice())).unwrap();
    let original = &rows[0]["transaction"];
    let raw: Bytes = original["input"].as_str().unwrap().parse().unwrap();
    let order = executeSwapIntentsCall::abi_decode_validate(&raw)
        .unwrap()
        .signedIntents
        .into_iter()
        .next()
        .unwrap()
        .intent;
    let router = original["to"].as_str().unwrap().parse().unwrap();
    assert_eq!(order.digest(8453, router), digest);
    let who = order.user;
    let dir = directory("ryze-429-recovery", 8453);
    let p = parts(&dir, 8453, who).await;
    p.ctx
        .settings
        .set(
            "dex.evm_chain_id",
            Scope::Venue(p.ctx.place.id.to_string()),
            Some(json!(8453)),
            "test",
        )
        .await
        .unwrap();
    let route = DexRoute {
        mode: RouteMode::Relay(digest),
        legs: vec![],
        block: 0,
        gas_units: 0,
        executable: true,
        exec_note: None,
    };
    let r = TradeRequest {
        venue: p.ctx.venue(),
        asset: Asset {
            chain: None,
            contract: Some(order.tokenOut),
            ticker: "WETH".into(),
        },
        side: Side::Buy,
        qty: Decimal::ONE,
        worst_price: Decimal::ONE,
        deal_id: "ryze-429".into(),
        route: Some(route),
        exit_position: false,
        remeasure: None,
    };
    let m = Marker {
        key: super::ledger::identity(&p.ctx, &r).unwrap(),
        account: p.ctx.account.clone().unwrap(),
        relayer: "https://mainnet.relayer.ryze.pro/api/v1".into(),
        router,
        runtime: B256::ZERO,
        digest,
        tokens: vec![],
        order,
        start: 0,
        start_hash: B256::ZERO,
        decimals: (18, 6),
    };
    let db = p.ctx.journal.db();
    intent::scope(
        db,
        &m.key,
        intent::Request::Order(Box::new(r.clone())),
        intent::before_send(&m.pending()),
    )
    .await
    .unwrap();
    assert!(m.reserve(db).await.unwrap());
    let http::Status::Limited(retry) = http::transaction(url("limited"), digest, limits()).await
    else {
        panic!("expected 429 on status")
    };
    budget::pause(db, retry, None).await.unwrap();
    assert!(!budget::submit_rejected(db, m.claim()).await.unwrap());
    let http::Submit::RateLimited(retry) =
        http::submit_once(url("limited"), digest, Value::Null, limits()).await
    else {
        panic!("expected 429 on submit")
    };
    assert_eq!(retry, Some(7));
    budget::pause(db, retry, Some(m.claim())).await.unwrap();
    drop(p);
    let p = parts(&dir, 8453, who).await;
    assert!(budget::limit(&p.ctx).is_none());
    for _ in 0..2 {
        let result = super::recovery::prior(&p, &r).await.unwrap().unwrap();
        assert_eq!(result.status, TradeStatus::NotFilled);
        assert_eq!(
            result.request.route.unwrap().exec_note.as_deref(),
            Some(budget::REJECTED)
        );
        assert_eq!(result.filled_qty, Decimal::ZERO);
        assert_eq!(result.venue_ref, digest.to_string());
    }
    assert!(m.holder(p.ctx.journal.db()).await.unwrap().is_none());
    run.abort();
}
