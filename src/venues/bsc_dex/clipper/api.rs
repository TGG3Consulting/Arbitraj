//! Официальный Blade API v2. Ключ остаётся в запросе к опубликованному HTTPS origin.
use super::{Ctx, ErrorCode, GatewayResult};
use crate::gateway::http;
use alloy_primitives::Address;
use reqwest::{Client, Url, header::HeaderValue};
use serde_json::Value;

pub(super) struct Api {
    base: Url,
    client: Client,
    auth: HeaderValue,
    chain: u64,
}
// Общий секрет Blade выдан этому origin. Перенаправления при запросе отключены.
pub(super) fn endpoint(raw: &str) -> Option<Url> {
    let url = Url::parse(raw).ok()?;
    (url.scheme() == "https" && url.host_str() == Some("blade-api.sushi.com")
        && url.port_or_known_default() == Some(443) && url.username().is_empty()
        && url.password().is_none() && url.query().is_none() && url.fragment().is_none())
        .then_some(url)
}
impl Api {
    pub fn new(ctx: &Ctx, chain: u64) -> GatewayResult<Self> {
        let base = endpoint(&ctx.text("clipper.base_url")?).ok_or_else(||
            ctx.err(ErrorCode::NotConfigured, "Укажите HTTPS URL официального Blade API в clipper.base_url"))?;
        let key = ctx.vault.get("clipper.api_key").map_err(|_| {
            ctx.err(
                ErrorCode::NotConfigured,
                "Откройте хранилище и укажите выданный Blade API ключ clipper.api_key",
            )
        })?;
        if key.trim().is_empty() {
            return Err(ctx.err(
                ErrorCode::NotConfigured,
                "Укажите ключ разрешённого доступа clipper.api_key",
            ));
        }
        let mut auth = HeaderValue::from_str(key.as_str()).map_err(|_| {
            ctx.err(
                ErrorCode::NotConfigured,
                "Некорректное значение clipper.api_key",
            )
        })?;
        auth.set_sensitive(true);
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .build()
            .map_err(|_| ctx.err(ErrorCode::Internal, "Не создан клиент Blade API"))?;
        Ok(Self {
            base,
            client,
            auth,
            chain,
        })
    }
    pub async fn get(
        &self,
        ctx: &Ctx,
        method: &str,
        args: &[(&str, String)],
    ) -> GatewayResult<Value> {
        let mut url = self.base.clone();
        url.path_segments_mut()
            .map_err(|_| ctx.err(ErrorCode::NotConfigured, "Некорректный путь Blade API"))?
            .pop_if_empty().push(method).push(&self.chain.to_string());
        url.query_pairs_mut().extend_pairs(args.iter().map(|(k, v)| (*k, v.as_str())));
        let request = self.client.get(url).header("x-api-key", self.auth.clone());
        let lim = ctx.limits()?;
        let (status, body) = http::exchange(request, lim).await.map_err(|e| {
            ctx.err(
                ErrorCode::BadResponse,
                format!("Blade {method}: {}", e.text(&lim)),
            )
        })?;
        if !status.is_success() {
            let why = match status.as_u16() {
                401 | 403 => "проверьте выданные права доступа",
                429 => "исчерпана квота запросов",
                _ => "источник не предоставил данные",
            };
            return Err(ctx.err(
                ErrorCode::BadResponse,
                format!("Blade {method}: HTTP {} — {why}", status.as_u16()),
            ));
        }
        serde_json::from_str(&body).map_err(|_| {
            ctx.err(
                ErrorCode::BadResponse,
                format!("Blade {method}: некорректный JSON"),
            )
        })
    }
}

pub(super) fn instruments(
    book: &Value,
    chain: u64,
    pool: Address,
    tokens: [Address; 2],
) -> Option<[String; 2]> {
    let addr = |v: &Value| v.as_str()?.parse::<Address>().ok();
    if book["pool_type"] != "onchain" {
        return None;
    }
    let mut entries = book["pools"].as_array()?.iter().filter(|r| {
        r["pool"]["chain_id"].as_u64() == Some(chain) && addr(&r["pool"]["address"]) == Some(pool)
    });
    let entry = entries.next()?;
    if entries.next().is_some() || entry["pool"]["swaps_enabled"] != true {
        return None;
    }
    let assets = entry["assets"].as_array()?;
    let name = |token| {
        let mut matches = assets.iter().filter(|a| addr(&a["address"]) == Some(token));
        let label = matches.next()?["name"].as_str()?;
        if matches.next().is_some()
            || label.is_empty()
            || assets
                .iter()
                .filter(|a| a["name"].as_str() == Some(label))
                .count()
                != 1
        {
            return None;
        }
        Some(label.to_owned())
    };
    Some([name(tokens[0])?, name(tokens[1])?])
}
