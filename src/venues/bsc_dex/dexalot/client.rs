//! Dexalot Simple Swap v2. pairprice uses token quantities; firm uses raw integer strings.
//! https://github.com/Dexalot/dexalot-docs/blob/a2a1ec62007cc2513a9099a8cec59a7da5487712/docs/en/apiv2/SimpleSwap.md
use super::fail;
use crate::venues::bsc_dex::quote::rfq_feed::Endpoint;
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
    let endpoint = Endpoint::available(&url, ctx)
        .ok_or_else(|| fail(ctx, "Dexalot: недоступна сеть или действует Retry-After"))?;
    let has_key = key_allowed(
        &url,
        firm.is_some() || method == super::channel::PRICES,
        ctx.vault.has_secret("dexalot.api_key"),
    )
    .map_err(|why| fail(ctx, why))?;
    append_path(&mut url, method, params).map_err(|why| fail(ctx, why))?;
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
    if has_key {
        let key = ctx
            .vault
            .get("dexalot.api_key")
            .map_err(|_| fail(ctx, "Dexalot: ключ канала недоступен"))?;
        let mut header = HeaderValue::from_str(key.trim())
            .map_err(|_| fail(ctx, "Dexalot: неверный формат ключа API"))?;
        header.set_sensitive(true);
        req = req.header("x-apikey", header);
    }
    let reply = http::exchange_full(req, ctx.limits()?)
        .await
        .map_err(|_| fail(ctx, "Dexalot: полный ответ не получен"))?;
    let status = reply.status;
    endpoint.record(&reply);
    decode(status, &reply.text).map_err(|why| fail(ctx, &why))
}

/// Собирает адрес одного метода с экранированием сегмента и параметров.
fn append_path(url: &mut Url, method: &str, params: &[(&str, String)]) -> Result<(), &'static str> {
    url.path_segments_mut()
        .map_err(|_| "Dexalot: недоступный путь API")?
        .pop_if_empty()
        .push(method);
    url.query_pairs_mut()
        .extend_pairs(params.iter().map(|(k, v)| (*k, v.as_str())));
    Ok(())
}

/// Только официальный endpoint может получить ключ. Проверка и резервирование требуют его обязательно.
fn key_allowed(url: &Url, required: bool, present: bool) -> Result<bool, &'static str> {
    let send = present && trusted(url);
    if required && !send {
        return Err("Dexalot: требуется сохранённый ключ и официальный HTTPS API");
    }
    Ok(send)
}

/// Доверенный адрес определяется поставляемой рекомендацией, без доступа к редактируемым настройкам.
fn trusted(url: &Url) -> bool {
    static ORIGINAL: OnceLock<Option<Url>> = OnceLock::new();
    ORIGINAL
        .get_or_init(|| {
            Url::parse(crate::settings::recommendations::venue_value(
                "dexalot.base_url",
                "base_dex",
            )?)
            .ok()
        })
        .as_ref()
        .is_some_and(|original| {
            url.scheme() == "https"
                && url.as_str().strip_suffix('/').unwrap_or(url.as_str()) == original.as_str()
        })
}

/// Возвращает полезные поля при успехе. В диагностику выходит только статус и известные коды.
pub(super) fn decode<T: DeserializeOwned>(
    status: reqwest::StatusCode,
    body: &str,
) -> Result<T, String> {
    let data: Value = serde_json::from_str(body)
        .map_err(|_| format!("Dexalot: HTTP {}, отсутствует JSON", status.as_u16()))?;
    if !status.is_success()
        || ["Success", "success"]
            .iter()
            .any(|key| data.get(*key) == Some(&Value::Bool(false)))
    {
        let reason = ["ReasonCode", "reasonCode", "code"]
            .iter()
            .find_map(|key| data.get(*key).and_then(Value::as_str));
        let message = match reason {
            Some("FQR-001") => "FQR-001, ключ канала не принят",
            Some("SQ-003") => "SQ-003, для указанного объёма недоступна котировка",
            Some("SQ-005") => "SQ-005, недостаточно ликвидности у maker",
            _ => "запрос отклонён",
        };
        return Err(format!("Dexalot: HTTP {}, {}", status.as_u16(), message));
    }
    serde_json::from_value(data).map_err(|_| "Dexalot: отсутствуют обязательные поля ответа".into())
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
