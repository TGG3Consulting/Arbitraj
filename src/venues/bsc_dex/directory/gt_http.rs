//! Файл: venues/bsc_dex/directory/gt_http.rs
//! Что делает: запросы к индексатору GeckoTerminal / CoinGecko on-chain по REST: адрес из index.api_url, ключ —
//! в заголовке index.key_header (секрет index.api_key), очередь — directory/limit.rs. Ответы — в сырой журнал
//! (data/raw/bsc_dex, соединение «index»). Страница за пределами списка (HTTP 400 на странице > 1) — «страниц больше
//! нет», а не ошибка.
//! Кто вызывает: directory/refresh.rs, directory/check.rs.
//! От чего зависит: ctx.rs, errors.rs, directory/limit.rs, directory/gt_parse.rs (текст ошибки).
//! Настройки: index.api_url, index.network, index.key_header [bsc_dex], http.timeout_ms; секрет index.api_key.
//!
//! Базовые адреса и заголовки ключа — https://docs.coingecko.com/docs/keyless-public-api (GeckoTerminal без ключа:
//! https://api.geckoterminal.com/api/v2; Demo — заголовок x-cg-demo-api-key, Pro — x-cg-pro-api-key) и
//! https://docs.coingecko.com/demo/reference/top-pools-contract-address (https://api.coingecko.com/api/v3 +
//! /onchain/networks/…). Пути после базового адреса у GeckoTerminal и CoinGecko /onchain одинаковые — платный
//! тариф = другой адрес + ключ. 400 на странице 3 списка DEX — записанный ответ 2026-10-24.

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::{Ctx, ID};
use super::super::errors;
use super::{gt_parse, limit};

/// Имя секрета ключа индексатора (тот же — secrets::KNOWN).
pub const SECRET_KEY: &str = "index.api_key";
/// Подпись источника для ошибок и сырого журнала.
pub const SOURCE: &str = "Индексатор";

/// Ответ индексатора.
#[derive(Debug)]
pub enum Got {
    /// 200 — тело ответа.
    Body(String),
    /// 400 на странице больше первой — страниц больше нет; текст ошибки индексатора.
    PageEnd(String),
}

/// Базовый адрес без «/» в конце и id сети. Ошибки: NotConfigured.
pub fn base(ctx: &Ctx) -> GatewayResult<(String, String)> {
    let url = ctx.text("index.api_url")?;
    let net = ctx.text("index.network")?;
    Ok((url.trim_end_matches('/').to_string(), net))
}

/// GET пути индексатора в очереди. Принимает: путь после базового адреса (с параметрами), номер страницы
/// (для «страниц больше нет»). Возвращает: Got. Ошибки: NotConfigured (адрес, сеть, темп, ключ при заданном
/// заголовке), SecretsLocked, сеть (Timeout / Unreachable), InvalidCredentials (401/403), RateLimited (429 после
/// правила), BadResponse (прочие коды — с текстом индексатора).
pub async fn get(ctx: &Ctx, path: &str, page: u32) -> GatewayResult<Got> {
    let (base, _) = base(ctx)?;
    let url = format!("{base}{path}");
    let key = match ctx.text("index.key_header") {
        Ok(h) => Some((h, ctx.vault.get(SECRET_KEY).map_err(|e| ctx.tag(e))?)),
        Err(e) if e.code == ErrorCode::NotConfigured => None,
        Err(e) => return Err(e),
    };
    let lim = ctx.limits()?;
    limit::run(ctx, || async {
        let mut req = ctx.http.get(&url).header("accept", "application/json");
        if let Some((h, k)) = &key {
            req = req.header(h.as_str(), k.as_str());
        }
        // Два срока (gateway/http.rs): до заголовков и тишина при чтении тела.
        let (status, text) = crate::gateway::http::exchange(req, lim)
            .await
            .map_err(|f| errors::from_fail(&f, &lim, SOURCE))?;
        ctx.raw
            .write(ID, "index", &format!("GET {url} → {status} {text}"));
        answer(status.as_u16(), page, &text, path)
    })
    .await
}

/// Разбор кода ответа. Принимает: HTTP-код, номер страницы, тело, путь (для текста). Возвращает: Got.
/// Ошибки: по коду (errors::from_http) с текстом индексатора errors[].title.
pub fn answer(status: u16, page: u32, text: &str, path: &str) -> GatewayResult<Got> {
    let title = gt_parse::error_title(text);
    match status {
        200 => Ok(Got::Body(text.to_string())),
        400 if page > 1 => Ok(Got::PageEnd(title.unwrap_or_default())),
        _ => {
            let mut e = errors::from_http(status, SOURCE, text);
            if matches!(status, 401 | 403) {
                e.message = format!(
                    "{SOURCE}: ключ не принят, HTTP {status} — проверьте ключ index.api_key и заголовок index.key_header"
                );
            }
            e.message = format!(
                "{} ({path}){}",
                e.message,
                title.map(|t| format!(": {t}")).unwrap_or_default()
            );
            Err(e)
        }
    }
}
