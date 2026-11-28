//! Файл: tests/common/kyber_http_pause.rs
//! Что делает: проверяет тело отказа, паузу одного origin и счётчики настоящего REST-пути KyberSwap.
//! Кто вызывает: tests/bsc_dex_best_of.rs. От чего зависит: записанные HTTP-ответы и изолированный Store.

use super::route_common;
use alloy_primitives::U256;
use arb::{
    gateway::{calc_meter, errors::ErrorCode, step_meter},
    settings::scope::Scope,
    venues::kyberswap::{ctx::Ctx, rest},
};
use axum::{Router, http::StatusCode, routing::get};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

/// У обоих адресов свой порт. Счётчик меняет только обработчик полученного HTTP-запроса.
async fn serve(chain: &str, status: StatusCode, body: String) -> (String, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let routes = Router::new().route(
        &format!("/{chain}/api/v1/routes"),
        get(move || {
            let body = body.clone();
            seen.fetch_add(1, Ordering::SeqCst);
            async move { (status, body) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, routes).await.unwrap() });
    (url, calls)
}

async fn setting(ctx: &Ctx, key: &str, value: Value) {
    ctx.settings
        .set(key, Scope::Global, Some(value), "О-72 тест")
        .await
        .unwrap();
}

async fn context(chain: &str, origin: &str) -> Ctx {
    let base = route_common::ctx().await;
    let ctx = Ctx {
        settings: base.settings,
        vault: base.vault,
        http: base.http,
        journal: base.journal,
        raw: base.raw,
        pace: Default::default(),
        wallet: None,
    };
    for (key, value) in [
        ("kyber.chain", json!(chain)),
        ("kyber.base_url", json!(origin)),
        ("kyber.rps", json!(100)),
        ("kyber.max_wait_ms", json!(1000)),
        ("kyber.http_timeout_ms", json!(5000)),
        ("http.body_idle_ms", json!(5000)),
        ("http.total_ms", json!(10000)),
        ("http.max_body_bytes", json!(1048576)),
    ] {
        setting(&ctx, key, value).await;
    }
    ctx
}

#[tokio::test]
async fn recorded_bsc_429_blocks_the_next_request_to_its_origin_without_step_logging() {
    let journal: Value = serde_json::from_str(include_str!(
        "../data/kyber_http_2026-11-28/bsc_status.json"
    ))
    .unwrap();
    let events = journal["entries"].as_array().unwrap();
    let message = events[0]["message"].as_str().unwrap();
    let code = message
        .split_once("HTTP ")
        .unwrap()
        .1
        .split_whitespace()
        .next()
        .unwrap();
    let status = StatusCode::from_bytes(code.as_bytes()).unwrap();
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);

    // Журнал сохранил только HTTP-код и причину. Здесь воспроизводим статус без заголовка Retry-After.
    let (origin, sent) = serve("bsc", status, String::new()).await;
    let (other, other_sent) = serve("bsc", status, String::new()).await;
    let ctx = context("bsc", &origin).await;
    let steps = step_meter::Rec::new();
    let costs = Arc::new(calc_meter::Meter::default());
    let qty = U256::from(1);
    step_meter::scope(
        steps.clone(),
        calc_meter::scope(costs.clone(), async {
            assert!(step_meter::active());
            let first = rest::routes(&ctx, route_common::USDT, route_common::CAKE, qty)
                .await
                .unwrap_err();
            assert_eq!(first.code, ErrorCode::RateLimited);
            for event in events {
                assert!(event["message"].as_str().unwrap().contains(&first.message));
            }
            assert_eq!(costs.snapshot().1.calls, 1);
            let held = rest::routes(&ctx, route_common::CAKE, route_common::USDT, qty)
                .await
                .unwrap_err();
            assert_eq!(held.code, ErrorCode::RateLimited);
            assert!(held.message.contains("пауза сервиса"));
            assert_eq!(sent.load(Ordering::SeqCst), 1);
            setting(&ctx, "kyber.base_url", json!(other)).await;
            let separate = rest::routes(&ctx, route_common::USDT, route_common::CAKE, qty)
                .await
                .unwrap_err();
            assert_eq!(separate.message, first.message);
            assert_eq!(other_sent.load(Ordering::SeqCst), 1);
        }),
    )
    .await;
    assert!(steps.take().is_empty());
}

#[tokio::test]
async fn archived_geckoterminal_429_body_survives_the_kyber_bsc_transport() {
    // У ответа GeckoTerminal сохранено полное тело 429. Используем его как HTTP-ответ стенда.
    let line = include_str!("../data/arbitrum_record_2026-11-20/logs/balancer_v3.log")
        .lines()
        .find(|line| {
            line.starts_with("stdhttp.HTTPError: HTTP 429 https://api.geckoterminal.com/")
                && line.contains("?page=4: ")
        })
        .unwrap();
    let (_, body) = line.split_once("?page=4: ").unwrap();
    let recorded: Value = serde_json::from_str(body).unwrap();
    assert_eq!(recorded["status"]["error_code"], 429);
    let (origin, sent) = serve("bsc", StatusCode::TOO_MANY_REQUESTS, body.to_owned()).await;
    let ctx = context("bsc", &origin).await;
    let steps = step_meter::Rec::new();
    step_meter::scope(steps.clone(), async {
        let request = || rest::routes(&ctx, route_common::USDT, route_common::CAKE, U256::from(1));
        let error = request().await.unwrap_err();
        assert_eq!(error.code, ErrorCode::RateLimited);
        assert_eq!(error.raw.as_deref(), Some(body));
        let held = request().await.unwrap_err();
        assert_eq!(held.code, ErrorCode::RateLimited);
        assert!(held.message.contains("пауза сервиса"));
        assert_eq!(sent.load(Ordering::SeqCst), 1);
    })
    .await;
    assert!(steps.take().is_empty());
}

#[tokio::test]
async fn archived_kyber_base_responses_keep_the_route_and_the_error_body() {
    let success: Value = serde_json::from_str(include_str!(
        "../data/kyber_http_2026-11-28/base-kyber-quote-open.json"
    ))
    .unwrap();
    let failure: Value = serde_json::from_str(include_str!(
        "../data/kyber_http_2026-11-28/base-kyber-quote-scoped.json"
    ))
    .unwrap();
    let summary = &success["response"]["data"]["routeSummary"];
    let input = summary["tokenIn"].as_str().unwrap().parse().unwrap();
    let output = summary["tokenOut"].as_str().unwrap().parse().unwrap();
    let qty = summary["amountIn"].as_str().unwrap().parse().unwrap();
    let expected = summary["amountOut"]
        .as_str()
        .unwrap()
        .parse::<U256>()
        .unwrap();
    let status = |v: &Value| {
        StatusCode::from_u16(v["status"].as_u64().unwrap().try_into().unwrap()).unwrap()
    };
    let (good, success_calls) =
        serve("base", status(&success), success["response"].to_string()).await;
    let body = failure["response"].to_string();
    let (refused, failure_calls) = serve("base", status(&failure), body.clone()).await;
    let ctx = context("base", &good).await;
    let steps = step_meter::Rec::new();
    step_meter::scope(steps.clone(), async {
        let route = rest::routes(&ctx, input, output, qty).await.unwrap();
        assert_eq!(route.amount_out, expected);
        assert_eq!(&route.summary, summary);
        setting(&ctx, "kyber.base_url", json!(refused)).await;
        // Поставщик записал этот отказ при includedSources=kaliber. Проверяем разбор его ответа.
        let error = rest::routes(&ctx, input, output, qty).await.unwrap_err();
        assert!(
            error
                .message
                .contains(failure["response"]["message"].as_str().unwrap())
        );
        assert_eq!(error.raw.as_deref(), Some(body.as_str()));
    })
    .await;
    assert!(steps.take().is_empty());
    assert_eq!(success_calls.load(Ordering::SeqCst), 1);
    assert_eq!(failure_calls.load(Ordering::SeqCst), 1);
}
