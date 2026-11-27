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
// Один зашифрованный секрет связывает получателя и ключ даже при смене настроек площадки.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Credential<'a> {
    origin: &'a str,
    key: &'a str,
}
fn https(raw: &str) -> Option<Url> {
    let url = Url::parse(raw).ok()?;
    (url.scheme() == "https" && url.host().is_some() && url.username().is_empty()
        && url.password().is_none() && url.query().is_none() && url.fragment().is_none()).then_some(url)
}
pub(super) fn scoped_key<'a>(raw_url: &str, raw_credential: &'a str) -> Option<(Url, &'a str)> {
    let credential: Credential<'a> = serde_json::from_str(raw_credential).ok()?;
    let base = https(raw_url)?;
    let issuer = https(credential.origin)?;
    (issuer.path() == "/" && issuer.origin() == base.origin() && !credential.key.trim().is_empty())
        .then_some((base, credential.key))
}
impl Api {
    pub fn new(ctx: &Ctx, chain: u64) -> GatewayResult<Self> {
        let key = ctx.vault.get("clipper.api_key").map_err(|_| {
            ctx.err(
                ErrorCode::NotConfigured,
                "Откройте хранилище и укажите выданный Blade API ключ clipper.api_key",
            )
        })?;
        let (base, token) = scoped_key(&ctx.text("clipper.base_url")?, &key).ok_or_else(||
            ctx.err(ErrorCode::NotConfigured, "HTTPS origin clipper.base_url и получатель ключа в clipper.api_key должны совпадать"))?;
        let mut auth = HeaderValue::from_str(token).map_err(|_| {
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
