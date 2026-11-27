//! Dexalot Simple Swap v2. pairprice uses token quantities; firm uses raw integer strings.
//! https://github.com/Dexalot/dexalot-docs/blob/a2a1ec62007cc2513a9099a8cec59a7da5487712/docs/en/apiv2/SimpleSwap.md
use super::fail;
use crate::{
    gateway::{errors::GatewayResult, http},
    venues::bsc_dex::ctx::Ctx,
};
use reqwest::{Client, Url, header::HeaderValue};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::sync::OnceLock;

pub(super) async fn request<T: DeserializeOwned>(
    ctx: &Ctx,
    method: &str,
    params: &[(&str, String)],
    firm: Option<&Value>,
) -> GatewayResult<T> {
    let base = ctx.text("dexalot.base_url")?;
    let mut url = Url::parse(&base).map_err(|_| fail(ctx, "Dexalot: некорректный HTTPS URL"))?;
    if url.scheme() != "https"
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(fail(
            ctx,
            "Dexalot: требуется HTTPS URL без пароля и параметров",
        ));
    }
    let original = url.as_str().trim_end_matches('/') == "https://api.dexalot.com/api/rfq";
    if firm.is_some() && !original {
        return Err(fail(ctx, "Dexalot: заявка доступна на официальном API"));
    }
    url.path_segments_mut()
        .map_err(|_| fail(ctx, "Dexalot: недоступный путь API"))?
        .pop_if_empty()
        .push(method);
    url.query_pairs_mut()
        .extend_pairs(params.iter().map(|(k, v)| (*k, v.as_str())));
    static CLIENT: OnceLock<Result<Client, reqwest::Error>> = OnceLock::new();
    let client = CLIENT
        .get_or_init(|| {
            Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .build()
        })
        .as_ref()
        .map_err(|_| fail(ctx, "Dexalot: не удалось открыть HTTPS"))?;
    let mut req = if let Some(body) = firm {
        client.post(url).json(body)
    } else {
        client.get(url)
    };
    if original && ctx.vault.has_secret("dexalot.api_key") {
        let key = ctx.vault.get("dexalot.api_key").map_err(|e| ctx.tag(e))?;
        let mut header = HeaderValue::from_str(key.trim())
            .map_err(|_| fail(ctx, "Dexalot: неверный формат ключа API"))?;
        header.set_sensitive(true);
        req = req.header("x-apikey", header);
    }
    let (status, body) = http::exchange(req, ctx.limits()?)
        .await
        .map_err(|_| fail(ctx, "Dexalot: полный ответ не получен"))?;
    let data: Value = serde_json::from_str(&body).map_err(|_| {
        fail(
            ctx,
            &format!("Dexalot: HTTP {}, отсутствует JSON", status.as_u16()),
        )
    })?;
    if !status.is_success() || data.get("success") == Some(&Value::Bool(false)) {
        let code = data
            .get("reasonCode")
            .or_else(|| data.get("code"))
            .and_then(Value::as_str)
            .filter(|c| c.len() <= 32 && c.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
            .unwrap_or("API");
        return Err(fail(
            ctx,
            &format!("Dexalot: HTTP {}, {}", status.as_u16(), code),
        ));
    }
    serde_json::from_value(data)
        .map_err(|_| fail(ctx, "Dexalot: отсутствуют обязательные поля ответа"))
}
