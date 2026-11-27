//! Архивный HTTP 429 каталога и записанная успешная цена. Заголовок Retry-After задаёт тестовый сервер.
use super::{Error, exchange};
use crate::gateway::http::Limits;
use axum::{Router, http::StatusCode, routing::get};
use std::{sync::{Arc, atomic::{AtomicUsize, Ordering}}, time::Duration};

#[tokio::test]
async fn market_read_respects_server_pause_without_blocking_other_services_or_sending_an_extra_request() {
    let catalog: serde_json::Value = serde_json::from_str(include_str!(
        "../../tests/data/first_run_dex_2026-11-26/catalog_recheck.json")).unwrap();
    let status = StatusCode::from_u16(catalog["base"]["pages"]["fluid-base"]["status"].as_u64().unwrap() as u16).unwrap();
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    let quotes: serde_json::Value = serde_json::from_str(include_str!(
        "../../tests/data/first_run_dex_2026-11-26/ryze_service_responses.json")).unwrap();
    let book = quotes["router_quotes"][1]["calls"][0]["body"].clone();
    assert!(book.is_object());
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let expected = book.clone();
    let service = Router::new()
        .route("/prices", get(move || {
            let count = count.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                (status, [("retry-after", "120")])
            }
        }))
        .route("/book", get(move || {
            let body = book.clone();
            async move { axum::Json(body) }
        }));
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", socket.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(socket, service).await.unwrap() });
    let client = reqwest::Client::new();
    let limits = Limits { head: Duration::from_secs(2), idle: Duration::from_secs(2),
        total: Duration::from_secs(5), max_bytes: 131_072 };
    assert_eq!(exchange("test-a", client.get(format!("{origin}/prices")), limits).await.unwrap().0, status);
    assert!(matches!(exchange("test-a", client.get(format!("{origin}/book")), limits).await,
        Err(Error::Held(left)) if left > Duration::from_secs(60)));
    let (ok, body) = exchange("test-b", client.get(format!("{origin}/book")), limits).await.unwrap();
    assert_eq!(ok, StatusCode::OK);
    assert_eq!(serde_json::from_str::<serde_json::Value>(&body).unwrap(), expected);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    task.abort();
}
