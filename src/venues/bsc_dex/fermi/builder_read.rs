//! Загружает публичный кадр строителя. Отправка подписанного бандла использует собственный канал.
use super::{Builder, Ctx};
use crate::gateway::{
    errors::{ErrorCode, GatewayResult},
    http::Limits,
    market_http,
};
use serde_json::{Value, json};

pub(super) async fn builder_snapshot(ctx: &Ctx, builder: Builder) -> GatewayResult<Value> {
    let (url, method) = builder.data();
    let mut limits = ctx.limits()?;
    limits.max_bytes = limits.max_bytes.min(1_048_576);
    fetch(ctx, url, method, limits).await
}

// Отдельный transport позволяет проверять чтение публичного кадра с записанным результатом и локальным HTTP.
async fn fetch(ctx: &Ctx, url: &str, method: &str, limits: Limits) -> GatewayResult<Value> {
    let request = ctx
        .http
        .post(url)
        .json(&json!({"jsonrpc":"2.0","id":1,"method":method,"params":[]}));
    let (status, body) = market_http::exchange("fermi", request, limits).await.map_err(|e| {
        ctx.err(
            if e.held() { ErrorCode::RateLimited } else { ErrorCode::Unreachable },
            e.description(&limits),
        )
    })?;
    let parsed = serde_json::from_str::<Value>(&body).ok();
    parsed
        .filter(|r| {
            status.is_success() && r["id"] == 1 && r["jsonrpc"] == "2.0" && r.get("error").is_none()
        })
        .and_then(|mut r| r.get_mut("result").map(Value::take))
        .ok_or_else(|| {
            ctx.err(
                ErrorCode::BadResponse,
                "Некорректный ответ строителя Ethereum",
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::Address;
    use axum::{Json, Router, http::StatusCode, response::IntoResponse, routing::post};
    use std::{sync::{Arc, atomic::{AtomicUsize, Ordering}}, time::Duration};

    #[tokio::test]
    async fn recorded_slot_and_429_use_the_same_read_endpoint() {
        let sample: Value = serde_json::from_str(include_str!(
            "../../../../tests/data/first_run_dex_2026-11-26/fermi_ethereum.json"
        )).unwrap();
        let frame = sample["frames"]["Titan"].clone();
        let method = frame["method"].as_str().unwrap().to_owned();
        assert_eq!(Builder::Titan.data().1, method);
        let result = json!({"venues":[{
            "maker":"Fermi", "quoteAddress":sample["router"], "blockNumber":frame["target"],
            "slot":frame["slot"], "stateOverride":{(sample["engine"].as_str().unwrap()):{"stateDiff":frame["stateDiff"]}}
        }]});
        let reply = result.clone();
        let expected = method.clone();
        let hits = Arc::new(AtomicUsize::new(0));
        let seen = hits.clone();
        let app = Router::new().route("/data", post(move |Json(req): Json<Value>| {
            let hit = seen.fetch_add(1, Ordering::SeqCst);
            let reply = reply.clone();
            let expected = expected.clone();
            async move {
                assert_eq!(req, json!({"jsonrpc":"2.0","id":1,"method":expected,"params":[]}));
                if hit == 0 {
                    Json(json!({"jsonrpc":"2.0","id":1,"result":reply})).into_response()
                } else {
                    // Заголовок задаёт тестовый сервер, поля кадра выше происходят из архива.
                    (StatusCode::TOO_MANY_REQUESTS, [("retry-after", "60")]).into_response()
                }
            }
        }));
        let socket = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let url = format!("http://{}/data", socket.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(socket, app).await.unwrap() });
        let dir = crate::venues::bsc_dex::hashflow::recovery_tests::directory("fermi-market-http", 1);
        let p = crate::venues::bsc_dex::hashflow::recovery_tests::parts(&dir, 1, Address::repeat_byte(1)).await;
        let limits = Limits {head: Duration::from_secs(2), total: Duration::from_secs(3),
            idle: Duration::from_secs(1), max_bytes: 1_048_576};
        let observed = fetch(&p.ctx, &url, &method, limits).await.unwrap();
        assert_eq!(observed, result);
        assert!(crate::venues::bsc_dex::fermi::frame::Frame::read(
            Builder::Titan, &observed, crate::venues::bsc_dex::fermi::tests::deployment()
        ).is_some());
        assert!(fetch(&p.ctx, &url, &method, limits).await.is_err());
        assert!(fetch(&p.ctx, &url, &method, limits).await.is_err());
        assert_eq!(hits.load(Ordering::SeqCst), 2);
        task.abort();
    }
}
