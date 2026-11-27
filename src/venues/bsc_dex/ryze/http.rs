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

#[derive(Debug, PartialEq, Eq)]
pub enum Submit {
    Accepted,
    RateLimited(Option<u64>),
    Unknown,
}

pub async fn submit_once(url: Url, id: B256, body: Value, limits: Limits) -> Submit {
    let Some(client) = client() else {
        return Submit::Unknown;
    };
    let Ok(reply) =
        http::exchange_status::<true>(client.post(url).json(&body), limited(limits, 8192)).await
    else {
        return Submit::Unknown;
    };
    if reply.status == StatusCode::TOO_MANY_REQUESTS {
        return Submit::RateLimited(reply.retry_after_s);
    }
    let accepted = reply.status == StatusCode::ACCEPTED
        && serde_json::from_str::<Value>(&reply.text).is_ok_and(|r| {
            r["success"] == true
                && r["status"] == "pending"
                && r["intentId"].as_str().and_then(|s| s.parse::<B256>().ok()) == Some(id)
        });
    if accepted {
        Submit::Accepted
    } else {
        Submit::Unknown
    }
}

pub enum Status {
    Limited(Option<u64>),
    Hint(Option<B256>),
}
/// Полученный txHash — подсказка. Даже 429 на GET ничего не говорит о принятом POST.
pub async fn transaction(url: Url, id: B256, limits: Limits) -> Status {
    let Some(client) = client() else {
        return Status::Hint(None);
    };
    let Ok(reply) = http::exchange_status::<true>(client.get(url), limited(limits, 8192)).await
    else {
        return Status::Hint(None);
    };
    if reply.status == StatusCode::TOO_MANY_REQUESTS {
        return Status::Limited(reply.retry_after_s);
    }
    let tx = serde_json::from_str::<Value>(&reply.text)
        .ok()
        .and_then(|r| {
            let intent = &r["intent"];
            if !reply.status.is_success()
                || r["success"] != true
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
        });
    Status::Hint(tx)
}
