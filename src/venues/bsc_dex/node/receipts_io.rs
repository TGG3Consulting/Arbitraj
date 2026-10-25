//! Файл: venues/bsc_dex/node/receipts_io.rs
//! Что делает: запросы ленты квитанций к узлу HTTP (решение 68): заголовок блока (hash, parentHash, хеши транзакций)
//! и квитанции блока со своими пределами ответа (bsc.receipts_max_body_bytes, bsc.receipts_total_ms — только для
//! квитанций) и проверкой полноты: квитанций столько же, сколько транзакций в заголовке, по порядку тех же хешей, все —
//! этого блока. Неполные, чужого хеша, больше предела — повтор того же блока (блоки не пропускаются). Ошибка одного
//! вызова внутри пачки (например, «слишком много запросов» с HTTP 200) — тоже повтор через bsc.receipts_retry_ms, не
//! правило RateLimited (оно — только на 429 всей пачки, pace.rs).
//! Там же — откат блока при реорге (rewind: снимается, только если хеш у узла другой по двум запросам).
//! Кто вызывает: node/receipts_task.rs.
//! От чего зависит: rpc.rs, rpc_capped.rs (очередь узла, класс задачи), receipts_chain.rs (complete), ctx.rs.
//! Настройки: bsc.receipts_max_body_bytes, bsc.receipts_total_ms [вся система]; http.timeout_ms, http.body_idle_ms.
//!
//! eth_getBlockByNumber (false — transactions как хеши) — execution-apis src/eth/block.yaml; eth_getBlockReceipts —
//! https://docs.bnbchain.org/bnb-smart-chain/developers/json_rpc/json-rpc-endpoint/ .

use std::time::Duration;

use alloy_primitives::B256;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::gateway::http::{Fail as HttpFail, Limits};

use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::receipts::{Feed, SOURCE};
use super::receipts_chain::{Chain, complete, gone};
use super::rpc;
use super::rpc_capped::{self, Capped};

/// Заголовок блока: только нужные поля.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Header {
    pub hash: B256,
    pub parent_hash: B256,
    /// Хеши транзакций блока (eth_getBlockByNumber с false).
    #[serde(default)]
    pub transactions: Vec<B256>,
}

/// Чем кончился шаг.
pub(super) enum Fail {
    /// Повторить тот же блок через bsc.receipts_retry_ms.
    Retry(String),
    /// Обрыв ленты.
    Break(String),
}

/// Заголовок блока. Ошибки: Retry — узел не ответил или блока ещё нет.
pub(super) async fn header(ctx: &Ctx, n: u64) -> Result<Header, Fail> {
    let h: Option<Header> = rpc::call(
        ctx,
        "eth_getBlockByNumber",
        json!([format!("0x{n:x}"), false]),
    )
    .await
    .map_err(|e| Fail::Retry(e.message))?;
    h.ok_or_else(|| Fail::Retry(format!("узел ещё не отдал блок {n}")))
}

/// Блок для конвейера (решение 69): заголовок и квитанции одним JSON-RPC batch (два вызова в одном HTTP, пределы —
/// как у квитанций), затем проверка полноты квитанций по заголовку. Принимает: окружение (своё — запрос идёт
/// отдельной задачей), номер. Возвращает: заголовок и квитанции. Ошибки: Break — не заданы пределы; Retry — узел не
/// ответил, блока ещё нет, ответ больше предела, неполные или чужие квитанции.
pub(super) async fn fetch_block(ctx: Ctx, n: u64) -> Result<(Header, Vec<Value>), Fail> {
    let lim = limits(&ctx)?;
    let tag = format!("0x{n:x}");
    let calls = [
        ("eth_getBlockByNumber", json!([tag, false])),
        ("eth_getBlockReceipts", json!([tag])),
    ];
    let got = rpc_capped::batch(&ctx, &calls, lim)
        .await
        .map_err(|e| Fail::Retry(e.message))?;
    let list = match got {
        Capped::Got(l) => l,
        Capped::Over(f) => return Err(Fail::Retry(over(n, &f, &lim))),
    };
    let (h, r) = pair(n, list).map_err(Fail::Retry)?;
    complete(n, h.hash, &h.transactions, &r).map_err(Fail::Retry)?;
    Ok((h, r))
}

/// Итоги пачки «заголовок, квитанции» → (заголовок, квитанции). Принимает: номер, итоги по порядку вызовов.
/// Ошибки (текст): ошибка узла по вызову, блока или квитанций ещё нет, ответ не по форме.
pub fn pair(
    n: u64,
    list: Vec<crate::gateway::errors::GatewayResult<Box<serde_json::value::RawValue>>>,
) -> Result<(Header, Vec<Value>), String> {
    let mut it = list.into_iter();
    let (Some(h), Some(r)) = (it.next(), it.next()) else {
        return Err(format!("блок {n}: в ответе на пачку не два итога"));
    };
    let h: Option<Header> = serde_json::from_str(h.map_err(|e| e.message)?.get())
        .map_err(|e| format!("блок {n}: заголовок не по форме: {e}"))?;
    let r: Option<Vec<Value>> = serde_json::from_str(r.map_err(|e| e.message)?.get())
        .map_err(|e| format!("блок {n}: квитанции не по форме: {e}"))?;
    let h = h.ok_or_else(|| format!("узел ещё не отдал блок {n}"))?;
    let r = r.ok_or_else(|| format!("узел ещё не отдал квитанции блока {n}"))?;
    Ok((h, r))
}

/// Пределы запроса квитанций: срок до заголовков и тишина тела — общие (http.timeout_ms, http.body_idle_ms), общий
/// срок и размер — свои. Ошибки: Break — настройка не задана.
fn limits(ctx: &Ctx) -> Result<Limits, Fail> {
    let base = ctx.limits().map_err(|e| Fail::Break(e.message))?;
    let set = |name: &str| ctx.int(name).map_err(|e| Fail::Break(e.message));
    Ok(Limits {
        total: Duration::from_millis(set("bsc.receipts_total_ms")?),
        max_bytes: usize::try_from(set("bsc.receipts_max_body_bytes")?).unwrap_or(usize::MAX),
        ..base
    })
}

/// Текст «квитанции больше предела». Ошибок нет.
fn over(n: u64, f: &HttpFail, lim: &Limits) -> String {
    match f {
        HttpFail::TooBig { got } => format!(
            "блок {n}: квитанции больше предела bsc.receipts_max_body_bytes ({} байт), получено {got} байт",
            lim.max_bytes
        ),
        HttpFail::Total { got } => format!(
            "блок {n}: квитанции больше предела bsc.receipts_total_ms ({} мс), получено {got} байт",
            lim.total.as_millis()
        ),
        other => format!("блок {n}: {}", other.text(lim)),
    }
}

/// Реорг: снимает последний блок окна, если его хеш у узла другой по двум запросам заголовка; логи — подпискам с
/// removed: true, номер — на блок раньше (одним действием). Хеш подтвердился прежним — родитель блока `next` у узла
/// разошёлся с ним (ответ отставшего узла): повтор. Ошибки: Retry — узел или ложный реорг; Break — окно кончилось
/// (реорг глубже bsc.receipts_reorg_depth).
pub(super) async fn rewind(
    feed: &Feed,
    ctx: &Ctx,
    chain: &mut Chain,
    next: u64,
) -> Result<(), Fail> {
    let Some(top) = chain.top().cloned() else {
        return Err(Fail::Break(
            "реорг глубже bsc.receipts_reorg_depth — пулы недостоверны, лента заново".into(),
        ));
    };
    let first = header(ctx, top.number).await?.hash;
    if first == top.hash {
        return Err(Fail::Retry(format!(
            "родитель блока {next} не совпал с блоком {}, а сам блок у узла прежний — повтор",
            top.number
        )));
    }
    let second = header(ctx, top.number).await?.hash;
    if !gone(top.hash, first, second) {
        return Err(Fail::Retry(format!(
            "хеш блока {} у узла расходится между ответами — повтор",
            top.number
        )));
    }
    if let Some((n, removed)) = chain.pop_removed() {
        feed.deliver(&removed, n - 1);
        let msg = format!("{SOURCE}: реорг — блок {n} отменён, его события пулов отменены");
        ctx.log(Level::Warn, "venue.ws", msg, None).await;
    }
    if chain.top().is_none() {
        return Err(Fail::Break(
            "реорг глубже bsc.receipts_reorg_depth — пулы недостоверны, лента заново".into(),
        ));
    }
    Ok(())
}
