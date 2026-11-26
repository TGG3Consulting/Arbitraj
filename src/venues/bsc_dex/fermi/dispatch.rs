//! Единственная отправка приватного бандла в указанный блок выбранного строителя Ethereum.
//! https://docs.titanbuilder.xyz/api/eth_sendbundle · https://buildernet.org/docs/api
use super::{Builder, Ctx};
use crate::gateway::{
    errors::{ErrorCode, GatewayResult},
    http,
};
use alloy::{
    primitives::{B256, Bytes, keccak256},
    signers::{SignerSync, local::PrivateKeySigner},
};
use serde_json::{Value, json};
use std::{sync::OnceLock, time::Duration};

fn envelope(builder: Builder, raw: &Bytes, target: u64, timestamp: u64) -> Value {
    let mut args =
        json!({"txs":[raw],"blockNumber":format!("0x{target:x}"),"minTimestamp":timestamp});
    if builder == Builder::BuilderNet {
        args["maxTimestamp"] = json!(timestamp);
        args["version"] = json!("v2");
        args["desiredState"] = json!("AnyPosition");
        args["targetAddresses"] = json!([super::ENGINE]);
        args["targetSlots"] = json!([[]]);
    }
    json!({"jsonrpc":"2.0","id":1,"method":"eth_sendBundle","params":[args]})
}
pub(super) async fn send(
    ctx: &Ctx,
    builder: Builder,
    key: &PrivateKeySigner,
    raw: &Bytes,
    target: u64,
    timestamp: u64,
) -> GatewayResult<B256> {
    static CLIENT: OnceLock<Result<reqwest::Client, reqwest::Error>> = OnceLock::new();
    let client = CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .build()
        })
        .as_ref()
        .map_err(|_| {
            ctx.err(
                ErrorCode::Internal,
                "Fermi: клиент приватной отправки недоступен",
            )
        })?;
    let body = envelope(builder, raw, target, timestamp).to_string();
    let url = match builder {
        Builder::Titan => "https://rpc.titanbuilder.xyz",
        Builder::BuilderNet => "https://rpc.buildernet.org",
    };
    let mut request = client
        .post(url)
        .header(reqwest::header::CONTENT_TYPE, "application/json");
    if builder == Builder::BuilderNet {
        let hash = keccak256(body.as_bytes()).to_string();
        let signature = key.sign_message_sync(hash.as_bytes()).map_err(|_| {
            ctx.err(
                ErrorCode::Internal,
                "Fermi: не удалось подтвердить запрос BuilderNet",
            )
        })?;
        let mut auth =
            reqwest::header::HeaderValue::from_str(&format!("{}:{signature}", key.address()))
                .map_err(|_| {
                    ctx.err(ErrorCode::Internal, "Fermi: неверный заголовок BuilderNet")
                })?;
        auth.set_sensitive(true);
        request = request.header("X-Flashbots-Signature", auth);
    }
    let mut limits = ctx.limits()?;
    let remaining = timestamp
        .saturating_mul(1000)
        .saturating_sub(chrono::Utc::now().timestamp_millis().max(0) as u64);
    if ctx.chain_id() != Some(1)
        || !ctx.flag("fermi.trading").unwrap_or(false)
        || ctx.text("fermi.builder").as_deref().ok() != Some(builder.name())
    {
        return Err(ctx.err(
            ErrorCode::NotApplicable,
            "Fermi: разрешение или строитель изменились перед передачей бандла",
        ));
    }
    if remaining == 0 {
        return Err(ctx.err(ErrorCode::StaleData, "Окно строителя Fermi закрыто"));
    }
    limits.total = limits.total.min(Duration::from_millis(remaining));
    limits.max_bytes = limits.max_bytes.min(8192);
    let (status, body) = http::exchange(request.body(body), limits)
        .await
        .map_err(|_| {
            ctx.err(
                ErrorCode::OutcomeUnknown,
                "Нет подтверждения получения приватного бандла Fermi",
            )
        })?;
    let response: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    response["result"]["bundleHash"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .filter(|h: &B256| {
            !h.is_zero()
                && status.is_success()
                && response["id"] == 1
                && response["error"].is_null()
        })
        .ok_or_else(|| {
            ctx.err(
                ErrorCode::OutcomeUnknown,
                "Строитель не подтвердил идентификатор бандла Fermi",
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn both_relays_receive_one_copy_for_exact_block_with_no_reverts() {
        let raw = Bytes::from(vec![2, 1]);
        for builder in [Builder::Titan, Builder::BuilderNet] {
            let e = envelope(builder, &raw, 421, 789);
            let b = &e["params"][0];
            assert_eq!(e["method"], "eth_sendBundle");
            assert_eq!(b["txs"], json!(["0x0201"]));
            assert_eq!(b["blockNumber"], "0x1a5");
            assert_eq!(b["minTimestamp"], 789);
            assert!(b.get("revertingTxHashes").is_none());
            assert_eq!(
                b.get("maxTimestamp").is_some(),
                builder == Builder::BuilderNet
            );
        }
    }
}
