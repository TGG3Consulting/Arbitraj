//! Файл: venues/bsc_dex/node/receipts_nodes.rs
//! Что делает: распределение запросов ленты квитанций по нескольким публичным узлам HTTP (решение 72) — чтобы поднять
//! пропускную способность, а не только как резерв для гонки. Список — настройка bsc.receipts_node_urls (адреса из
//! официального списка публичных узлов BNB Chain, задаёт Тигран), темп каждого — bsc.receipts_node_rps (у каждого
//! публичного узла свой лимит). Блок n уходит на узел n по кругу (соседние блоки конвейера — на разные узлы), повтор
//! блока, на котором стоит лента, — гонкой на все узлы списка. Список не задан — как раньше: основной узел
//! (bsc.node_http_url) через общую очередь bsc.node_http_rps, гонка — основной и резервный.
//! Кто вызывает: node/receipts_io.rs (fetch_block).
//! От чего зависит: ctx.rs, rpc_race.rs (one, first_accepted), rpc_capped.rs (Capped).
//! Настройки: bsc.receipts_node_urls, bsc.receipts_node_rps [вся система].
//!
//! Официальный список публичных узлов и их лимит — https://docs.bnbchain.org/bnb-smart-chain/developers/json_rpc/json-rpc-endpoint/ .

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use tokio::time::Instant;

use crate::gateway::errors::GatewayResult;
use crate::settings::read::At;

use super::super::ctx::Ctx;

/// Темп по узлам списка: когда каждый узел свободен для следующего вызова.
#[derive(Default)]
pub struct Nodes {
    free: Mutex<HashMap<String, Instant>>,
}

/// Список узлов и темп каждого. None — список не задан или пуст (лента идёт через основной узел). Ошибки: список задан,
/// а темп нет (или 0), адрес не https.
pub fn list(ctx: &Ctx) -> GatewayResult<Option<(Vec<String>, u64)>> {
    let urls = match ctx.settings.list("bsc.receipts_node_urls", At::default()) {
        Ok(v) if !v.is_empty() => v,
        _ => return Ok(None),
    };
    if let Some(bad) = urls.iter().find(|u| !u.starts_with("https://")) {
        return Err(ctx.not_set(
            "bsc.receipts_node_urls",
            format!("bsc.receipts_node_urls: адрес {bad} — не https"),
        ));
    }
    let rps = ctx
        .settings
        .integer("bsc.receipts_node_rps", At::default())?;
    if rps == 0 {
        return Err(ctx.not_set(
            "bsc.receipts_node_rps",
            "bsc.receipts_node_rps должна быть больше нуля",
        ));
    }
    Ok(Some((urls, rps)))
}

/// Узел для блока `n`: по кругу. Ошибок нет (список не пуст — проверено в list).
pub fn pick(urls: &[String], n: u64) -> &str {
    &urls[(n % urls.len() as u64) as usize]
}

impl Nodes {
    /// Ждёт своей очереди у узла `url`: `weight` вызовов при темпе `rps` в секунду. Ошибок нет.
    pub async fn wait(&self, url: &str, weight: usize, rps: u64) {
        let at = self.reserve(url, weight, rps, Instant::now());
        tokio::time::sleep_until(at).await;
    }

    /// Занимает место у узла `url` на момент `now`: возвращает, когда можно слать (не раньше `now` и не раньше конца
    /// прошлого занятия этого узла), и сдвигает его занятость на `weight` / `rps` секунд. Другие узлы не трогаются.
    /// Ошибок нет.
    pub fn reserve(&self, url: &str, weight: usize, rps: u64, now: Instant) -> Instant {
        let step = Duration::from_secs_f64(weight as f64 / rps.max(1) as f64);
        let mut free = self.free.lock().unwrap_or_else(|p| p.into_inner());
        let slot = free.entry(url.to_string()).or_insert(now);
        let at = (*slot).max(now);
        *slot = at + step;
        at
    }
}
