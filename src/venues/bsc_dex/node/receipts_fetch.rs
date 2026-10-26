//! Файл: venues/bsc_dex/node/receipts_fetch.rs
//! Что делает: запрос блока для конвейера ленты квитанций (решения 69, 71, 72): заголовок и квитанции одним JSON-RPC
//! batch, проверка полноты (receipts_chain.rs complete) — внутри запроса, так что гонка на все узлы берёт первый
//! ПОЛНЫЙ ответ. Узлы: список bsc.receipts_node_urls (блок — на свой узел по кругу, свой темп; receipts_nodes.rs) или,
//! если списка нет, основной узел через общую очередь bsc.node_http_rps (гонка — основной и резервный). Блока нет ни у
//! одного узла — Missing (receipts_task.rs решает, невосполним ли пропуск).
//! Кто вызывает: node/receipts_task.rs; tests/bsc_dex_receipts_pipe.rs (pair на записанном ответе).
//! От чего зависит: receipts_io.rs (Header, Fail, limits, over), rpc_capped.rs, rpc_race.rs, receipts_nodes.rs,
//! receipts_chain.rs, jsonrpc.rs.
//! Настройки: через вызываемые — bsc.receipts_max_body_bytes, bsc.receipts_total_ms, bsc.receipts_node_urls,
//! bsc.receipts_node_rps.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::gateway::http::Limits;

use super::super::ctx::Ctx;
use super::receipts_chain::complete;
use super::receipts_io::{Fail, Header, limits, over};
use super::receipts_nodes::{self, Nodes};
use super::rpc_capped::{self, Capped};
use super::rpc_race::{self, Lost, Reject};

/// Начало причины «блока у узла ещё нет» (pair) — по ней ответ отличается от неполного или ошибки. Ошибок нет.
pub const MISSING: &str = "узел ещё не отдал";

/// Блок для конвейера (решение 69): заголовок, квитанции и голова узла (eth_blockNumber) одним JSON-RPC batch — три
/// вызова в одном HTTP, пределы — как у квитанций; полнота квитанций проверяется внутри запроса (гонка берёт первый
/// ПОЛНЫЙ ответ). Голова узла — чтобы отличить «узел ещё не дошёл до блока» (публичные узлы за балансировщиком отстают
/// от основного на 1–3 блока) от «узел дошёл, а блок не отдаёт» (решение 72). Принимает: окружение (своё — запрос
/// идёт отдельной задачей), темп узлов списка, номер, гонку (повтор — сразу все узлы). Возвращает: заголовок и
/// квитанции. Ошибки: Break — не заданы пределы; Missing — гонка: блок не отдал ни один узел, хотя у каждого голова
/// уже не ниже блока; Retry — остальное (узел не ответил или ещё не дошёл, ответ больше предела, неполные квитанции).
pub(super) async fn fetch_block(
    ctx: Ctx,
    nodes: Arc<Nodes>,
    n: u64,
    race: bool,
) -> Result<(Header, Vec<Value>), Fail> {
    let lim = limits(&ctx)?;
    // Список задан неверно (исправили между проверкой задачи ленты и запросом) — повтор; задача ленты на следующем
    // шаге оборвёт ленту один раз и будет ждать исправления (receipts_task.rs).
    let spread = receipts_nodes::list(&ctx).map_err(|e| Fail::Retry(e.message))?;
    let tag = format!("0x{n:x}");
    let calls = [
        ("eth_getBlockByNumber", json!([tag, false])),
        ("eth_getBlockReceipts", json!([tag])),
        ("eth_blockNumber", json!([])),
    ];
    let check = |list: Vec<_>| {
        let head = node_head(&list);
        let reject = |why: String| Reject { why, head };
        let (h, r) = pair(n, list).map_err(reject)?;
        complete(n, h.hash, &h.transactions, &r).map_err(reject)?;
        Ok((h, r))
    };
    // Список публичных узлов (решение 72): блок — на свой узел по кругу со своим темпом, повтор — гонкой на все.
    if let Some((urls, rps)) = spread {
        let body = Value::Array(
            calls
                .iter()
                .enumerate()
                .map(|(i, (m, p))| super::jsonrpc::request(i as u64, m, p))
                .collect(),
        );
        let weight = calls.len();
        let run = |url: String| {
            let (ctx, nodes, body) = (ctx.clone(), nodes.clone(), body.clone());
            async move {
                nodes.wait(&url, weight, rps).await;
                rpc_race::one(&ctx, &url, &body, lim).await
            }
        };
        let runs: Vec<_> = if race {
            urls.iter().cloned().map(run).collect()
        } else {
            vec![run(receipts_nodes::pick(&urls, n).to_string())]
        };
        return rpc_race::first_accepted(runs, calls.len(), check)
            .await
            .map_err(|l| lost(n, l, &lim, race));
    }
    // Повтор блока, на котором стоит конвейер, — сразу на все узлы; побеждает первый ПОЛНЫЙ ответ (решение 71).
    if race {
        return rpc_race::batch_race(&ctx, &calls, lim, check)
            .await
            .map_err(|l| lost(n, l, &lim, true));
    }
    let got = rpc_capped::batch(&ctx, &calls, lim)
        .await
        .map_err(|e| Fail::Retry(e.message))?;
    match got {
        Capped::Got(list) => check(list).map_err(|r| Fail::Retry(r.why)),
        Capped::Over(f) => Err(Fail::Retry(over(n, &f, &lim))),
    }
}

/// Голова узла из третьего итога пачки (eth_blockNumber). None — нет или не по форме. Ошибок нет.
fn node_head(
    list: &[crate::gateway::errors::GatewayResult<Box<serde_json::value::RawValue>>],
) -> Option<u64> {
    let raw = list.get(2)?.as_ref().ok()?;
    let v: alloy_primitives::U64 = serde_json::from_str(raw.get()).ok()?;
    Some(v.to())
}

/// Пропуск блока `n` невосполним: ни один узел его не отдал (никто не упал и не превысил предел), каждый ответил
/// «блока нет», и у каждого СВОЯ голова не ниже `n` (узел дошёл до блока, а отдать не может). Голова узла ниже `n` или
/// неизвестна — узел просто отстаёт: повтор. Ошибок нет.
pub fn nowhere(l: &Lost, n: u64) -> bool {
    l.over.is_none()
        && l.failed.is_none()
        && !l.rejected.is_empty()
        && l.rejected
            .iter()
            .all(|r| r.why.starts_with(MISSING) && r.head.is_some_and(|h| h >= n))
}

/// Проигранный запрос → итог блока: гонка и блока нет нигде (nowhere) — Missing; иначе — повтор с причиной (у
/// причины «блока нет» — голова узла). Ошибок нет.
pub fn lost(n: u64, l: Lost, lim: &Limits, race: bool) -> Fail {
    if race && nowhere(&l, n) {
        let why: Vec<String> = l.rejected.iter().map(|r| r.why.clone()).collect();
        return Fail::Missing(why.join("; "), l.rejected.len());
    }
    if let Some(r) = l.rejected.last() {
        return Fail::Retry(match r.head {
            Some(h) if h < n => format!("{} (голова узла {h} ещё ниже блока)", r.why),
            _ => r.why.clone(),
        });
    }
    match (l.over, l.failed) {
        (Some(f), _) => Fail::Retry(over(n, &f, lim)),
        (None, Some(e)) => Fail::Retry(e.message),
        (None, None) => Fail::Retry(format!("блок {n}: узлов нет")),
    }
}

/// Итоги пачки «заголовок, квитанции[, голова]» → (заголовок, квитанции). Принимает: номер, итоги по порядку вызовов.
/// Ошибки (текст): ошибка узла по вызову, блока или квитанций ещё нет, ответ не по форме.
pub fn pair(
    n: u64,
    list: Vec<crate::gateway::errors::GatewayResult<Box<serde_json::value::RawValue>>>,
) -> Result<(Header, Vec<Value>), String> {
    let mut it = list.into_iter();
    let (Some(h), Some(r)) = (it.next(), it.next()) else {
        return Err(format!(
            "блок {n}: в ответе на пачку нет заголовка и квитанций"
        ));
    };
    let h: Option<Header> = serde_json::from_str(h.map_err(|e| e.message)?.get())
        .map_err(|e| format!("блок {n}: заголовок не по форме: {e}"))?;
    let r: Option<Vec<Value>> = serde_json::from_str(r.map_err(|e| e.message)?.get())
        .map_err(|e| format!("блок {n}: квитанции не по форме: {e}"))?;
    let h = h.ok_or_else(|| format!("{MISSING} блок {n}"))?;
    let r = r.ok_or_else(|| format!("{MISSING} квитанции блока {n}"))?;
    Ok((h, r))
}
