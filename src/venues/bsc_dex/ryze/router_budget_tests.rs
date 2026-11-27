//! Успех /quote воспроизводит архив. HTTP 429 и Retry-After моделирует локальный сервер.
use super::{budget, http, relay_tests::limits};
use crate::{settings::scope::Scope, venues::bsc_dex::{hashflow::recovery_tests, parts::Parts}};
use alloy::primitives::Address;
use axum::{Json, Router, http::StatusCode, response::IntoResponse, routing::post};
use reqwest::Url;
use serde_json::{Value, json};
use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};

async fn configure(p: &Parts, chain: u64, max: u64) {
    for (name, scope, value) in [
        ("dex.evm_chain_id", Scope::Venue(p.ctx.place.id.to_owned()), chain),
        ("ryze.router_requests_per_minute", Scope::Global, max),
        ("ryze.relayer_requests_per_minute", Scope::Global, 1),
    ] {
        p.ctx.settings.set(name, scope, Some(json!(value)), "test").await.unwrap();
    }
}
pub(super) async fn parts(tag: &str, wallet: Address, max: u64) -> Parts {
    let chain = super::CHAIN_ID;
    let dir = recovery_tests::directory(tag, chain);
    let p = recovery_tests::parts(&dir, chain, wallet).await;
    configure(&p, chain, max).await;
    p
}
fn recorded() -> Value {
    let data: Value = serde_json::from_str(include_str!(
        "../../../../tests/data/first_run_dex_2026-11-26/ryze_service_responses.json"
    )).unwrap();
    data["router_quotes"][1]["calls"][0].clone()
}

async fn server(limited: bool) -> (Url, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    let row = recorded();
    let route = post(move |Json(body): Json<Value>| {
        seen.fetch_add(1, Ordering::SeqCst);
        let row = row.clone();
        async move {
            assert_eq!(body, row["request"]);
            if limited {
                (StatusCode::TOO_MANY_REQUESTS, [("retry-after", "120")], vec![0xff]).into_response()
            } else {
                Json(row["body"].clone()).into_response()
            }
        }
    });
    let app = Router::new().route("/quote", route.clone()).route("/another/quote", route);
    let sock = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let url = Url::parse(&format!("http://{}/quote", sock.local_addr().unwrap())).unwrap();
    let task = tokio::spawn(async move { axum::serve(sock, app).await.unwrap() });
    (url, count, task)
}

#[tokio::test]
async fn two_wallets_share_a_single_origin_and_get_the_recorded_quote_once() {
    let (url, hits, server) = server(false).await;
    let chain = super::CHAIN_ID;
    let dir = recovery_tests::directory("ryze-origin-budget", chain);
    let a = recovery_tests::parts(&dir, chain, Address::repeat_byte(1)).await;
    configure(&a, chain, 1).await;
    let b = recovery_tests::parts(&dir, chain, Address::repeat_byte(2)).await;
    let row = recorded();
    let request = &row["request"];
    let other_path = url.join("/another/quote").unwrap();
    let (one, two) = tokio::join!(
        http::quote(&a.ctx, &url, request, limits()),
        http::quote(&b.ctx, &other_path, request, limits())
    );
    assert_ne!(one.is_some(), two.is_some());
    assert_eq!(one.or(two), Some(row["body"].clone()));
    assert!(!budget::router_ready(&b.ctx, &url.origin().ascii_serialization()));
    b.ctx.settings.set("ryze.router_requests_per_minute", Scope::Global, Some(json!(2)), "test").await.unwrap();
    assert!(budget::router_ready(&b.ctx, &url.origin().ascii_serialization()));
    assert!(budget::take(&b.ctx).await.is_ok());
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    server.abort();
}

#[tokio::test]
async fn router_429_survives_restart_and_other_origin_and_relayer_stay_available() {
    let (url, hits, throttled) = server(true).await;
    let (other, other_hits, healthy) = server(false).await;
    let chain = super::CHAIN_ID;
    let dir = recovery_tests::directory("ryze-router-pause", chain);
    let owner = Address::repeat_byte(3);
    let row = recorded();
    let req = &row["request"];
    {
        let p = recovery_tests::parts(&dir, chain, owner).await;
        assert!(http::quote(&p.ctx, &url, req, limits()).await.is_none());
        let bsc = recovery_tests::parts(&dir, 56, owner).await;
        configure(&bsc, 56, 10).await;
        assert!(http::quote(&bsc.ctx, &url, req, limits()).await.is_none());
        drop(bsc);
        configure(&p, chain, 10).await;
        let p = recovery_tests::parts(&dir, chain, owner).await;
        assert_eq!(hits.load(Ordering::SeqCst), 0);
        assert!(http::quote(&p.ctx, &url, req, limits()).await.is_none());
        let key = url.origin().ascii_serialization();
        let until: i64 = p.ctx.journal.db().ledger().read(move |c| c.query_row(
            "SELECT json_extract(body,'$.blocked_until') FROM core_state WHERE kind='ryze_router_window' AND id=?1",
            [key], |r| r.get(0)
        )).await.unwrap();
        assert!(until > chrono::Utc::now().timestamp_millis() + 60_000);
    }
    super::budget_window::forget_router(&url.origin().ascii_serialization());
    let p = recovery_tests::parts(&dir, chain, owner).await;
    assert!(budget::router_ready(&p.ctx, &url.origin().ascii_serialization()));
    assert!(http::quote(&p.ctx, &url, req, limits()).await.is_none());
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert!(!budget::router_ready(&p.ctx, &url.origin().ascii_serialization()));
    assert_eq!(http::quote(&p.ctx, &other, req, limits()).await, Some(row["body"].clone()));
    assert_eq!(other_hits.load(Ordering::SeqCst), 1);
    assert!(budget::take(&p.ctx).await.is_ok());
    throttled.abort();
    healthy.abort();
}
