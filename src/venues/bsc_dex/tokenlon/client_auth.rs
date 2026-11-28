//! Личная подпись времени из SDK и краткоживущий JWT только для официального SockJS origin.
//! https://github.com/consenlabs/tokenlon-v5-sdk · Signer.signAuthToken и Quoter.auth/connect.
use super::Failure;
use crate::{
    gateway::{http::Limits, market_http},
    venues::bsc_dex::ctx::Ctx,
};
use alloy::signers::{SignerSync, local::PrivateKeySigner};
use serde_json::{Value, json};
use std::sync::OnceLock;
use url::Url;
use zeroize::Zeroizing;

const PUBLISHER: &str = "https://publisher.tokenlon.im/rpc";
pub(super) fn allowed(endpoint: &str) -> bool {
    endpoint == PUBLISHER
}

pub(super) fn signed(timestamp: u64, key: &PrivateKeySigner) -> Result<Value, Failure> {
    let signature = key
        .sign_message_sync(timestamp.to_string().as_bytes())
        .map_err(|_| Failure::Credentials)?;
    Ok(
        json!({"jsonrpc":"2.0","id":1,"method":"auth.getSdkJwtToken","params":{"timestamp":timestamp,"signature":signature.to_string()}}),
    )
}
pub(super) fn socket_url(token: &str) -> Result<Url, Failure> {
    if token.is_empty()
        || !token.is_ascii()
        || token.bytes().any(|b| b.is_ascii_control() || b == b' ')
    {
        return Err(Failure::Credentials);
    }
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| Failure::Transport)?;
    let server = u16::from_be_bytes([bytes[0], bytes[1]]) % 1000;
    let mut url = Url::parse(&format!(
        "wss://publisher.tokenlon.im/exchange/{server:03}/{}/websocket",
        hex::encode(&bytes[2..])
    ))
    .map_err(|_| Failure::Endpoint)?;
    url.query_pairs_mut()
        .append_pair("Authorization", &format!("Token {token}"));
    Ok(url)
}

pub(super) async fn websocket(
    ctx: &Ctx,
    key: &PrivateKeySigner,
    limits: Limits,
) -> Result<Url, Failure> {
    static CLIENT: OnceLock<Result<reqwest::Client, reqwest::Error>> = OnceLock::new();
    let client = CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
        })
        .as_ref()
        .map_err(|_| Failure::Transport)?;
    let msg = signed(chrono::Utc::now().timestamp().max(0) as u64, key)?;
    let req = client.post(PUBLISHER).json(&msg);
    let (status, body) = market_http::exchange_unmetered("Tokenlon", req, limits)
        .await
        .map_err(|e| {
            if e.held() {
                Failure::Throttle
            } else {
                Failure::Transport
            }
        })?;
    if status.as_u16() == 429 {
        return Err(Failure::Throttle);
    }
    if !status.is_success() {
        return Err(if matches!(status.as_u16(), 401 | 403) {
            Failure::Credentials
        } else {
            Failure::Transport
        });
    }
    let body = Zeroizing::new(body);
    #[derive(serde::Deserialize)]
    struct Session<'a> {
        id: u64,
        #[serde(borrow)]
        result: Option<&'a str>,
    }
    let reply: Session<'_> = serde_json::from_str(&body).map_err(|_| Failure::Credentials)?;
    if reply.id != 1 || !super::super::publisher_network(ctx.chain_id()) {
        return Err(Failure::Credentials);
    }
    socket_url(reply.result.ok_or(Failure::Credentials)?)
}
