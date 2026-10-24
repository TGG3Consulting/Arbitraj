//! Файл: venues/bsc_dex/directory/check.rs
//! Что делает: часть «Проверить подключение» DEX для индексатора справочника пулов: первая страница списка DEX сети
//! (GET /networks/{network}/dexes?page=1) с заданным адресом, ключом и в той же очереди запросов. Ответ — сколько DEX
//! на странице и с ключом ли; сбой — точная причина (не задано, ключ не принят, лимит, сеть не найдена, нет связи).
//! Кто вызывает: connection.rs.
//! От чего зависит: ctx.rs, directory/gt_http.rs, directory/gt_parse.rs.
//! Настройки: index.api_url, index.network, index.key_header, index.requests_per_min [bsc_dex]; секрет index.api_key.
//!
//! Эндпоинт — https://docs.coingecko.com/demo/reference/endpoint-overview (GET /onchain/networks/{network}/dexes).

use super::super::ctx::Ctx;
use super::gt_http::{self, Got};
use super::gt_parse;

/// Проверка индексатора. Возвращает: описание или текст причины.
pub async fn check(ctx: &Ctx) -> Result<String, String> {
    let (base, net) = gt_http::base(ctx).map_err(|e| e.message)?;
    let path = format!("/networks/{net}/dexes?page=1");
    let body = match gt_http::get(ctx, &path, 1).await.map_err(|e| e.message)? {
        Got::Body(b) => b,
        Got::PageEnd(t) => return Err(format!("индексатор ответил 400: {t}")),
    };
    let (list, _) = gt_parse::dexes(&body).map_err(|w| format!("ответ {path}: {w}"))?;
    let key = if ctx.text("index.key_header").is_ok() {
        "с ключом"
    } else {
        "без ключа"
    };
    Ok(format!(
        "{base} отвечает {key}: сеть {net}, DEX на первой странице — {}",
        list.len()
    ))
}
