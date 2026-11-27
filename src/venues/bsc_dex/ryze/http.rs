//! Один запрос на отправку. При потере ответа восстановление использует только GET статуса и чтение блокчейна.
use crate::gateway::http::{self, Limits};
use alloy::primitives::B256;
use reqwest::{Client, StatusCode, Url};
use serde_json::Value;
use std::sync::OnceLock;

fn client() -> Option<&'static Client> {
    static CLIENT: OnceLock<Result<Client, reqwest::Error>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .build()
        })
        .as_ref()
        .ok()
}

pub fn endpoint(base: &str, suffix: &str) -> Option<Url> {
    let mut u = Url::parse(base).ok()?;
    if u.scheme() != "https"
        || u.host_str().is_none()
        || !u.username().is_empty()
        || u.password().is_some()
        || u.query().is_some()
        || u.fragment().is_some()
    {
        return None;
    }
    let path = format!("{}/{}", u.path().trim_end_matches('/'), suffix);
    u.set_path(&path);
    Some(u)
}

fn limited(mut limits: Limits, cap: usize) -> Limits {
    limits.max_bytes = limits.max_bytes.min(cap);
    limits
}

pub async fn quote(url: Url, body: &Value, limits: Limits) -> Option<Value> {
    let (status, response) =
        http::exchange(client()?.post(url).json(body), limited(limits, 131_072))
            .await
            .ok()?;
    if !status.is_success() {
        return None;
    }
    serde_json::from_str(&response).ok()
}

pub async fn submit_once(url: Url, id: B256, body: Value, limits: Limits) -> bool {
    let Some(client) = client() else {
        return false;
    };
    let Ok((status, text)) =
        http::exchange(client.post(url).json(&body), limited(limits, 8192)).await
    else {
        return false;
    };
    let Ok(resp) = serde_json::from_str::<Value>(&text) else {
        return false;
    };
    status == StatusCode::ACCEPTED
        && resp["success"] == true
        && resp["status"] == "pending"
        && resp["intentId"]
            .as_str()
            .and_then(|v| v.parse::<B256>().ok())
            == Some(id)
}

/// Полученный txHash — подсказка для поиска. Все остальные состояния оставляют поручение на канонической сверке.
pub async fn transaction(url: Url, id: B256, limits: Limits) -> Option<B256> {
    let (status, text) = http::exchange(client()?.get(url), limited(limits, 8192))
        .await
        .ok()?;
    let resp: Value = serde_json::from_str(&text).ok()?;
    let intent = &resp["intent"];
    if !status.is_success()
        || resp["success"] != true
        || intent["status"] != "confirmed"
        || intent["intentId"].as_str()?.parse::<B256>().ok()? != id
    {
        return None;
    }
    intent["txHash"]
        .as_str()?
        .parse::<B256>()
        .ok()
        .filter(|v| !v.is_zero())
}
