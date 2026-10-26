//! Файл: venues/bsc_dex/node/receipts_io.rs
//! Что делает: общее ленты квитанций (решение 68): заголовок блока (hash, parentHash, хеши транзакций), итог шага
//! (Fail: повтор, обрыв, блока нет), пределы ответа квитанций (bsc.receipts_max_body_bytes, bsc.receipts_total_ms —
//! только для квитанций) и текст превышения; откат блока при реорге (rewind: снимается, только если хеш у узла другой
//! по двум запросам). Запрос блока для конвейера — receipts_fetch.rs.
//! Кто вызывает: node/receipts_task.rs.
//! От чего зависит: rpc.rs, rpc_capped.rs (очередь узла, класс задачи), receipts_chain.rs (complete), ctx.rs.
//! Настройки: bsc.receipts_max_body_bytes, bsc.receipts_total_ms [вся система]; http.timeout_ms, http.body_idle_ms.
//!
//! eth_getBlockByNumber (false — transactions как хеши) — execution-apis src/eth/block.yaml; eth_getBlockReceipts —
//! https://docs.bnbchain.org/bnb-smart-chain/developers/json_rpc/json-rpc-endpoint/ .

use std::time::Duration;

use alloy_primitives::B256;
use serde::Deserialize;
use serde_json::json;

use crate::gateway::http::{Fail as HttpFail, Limits};

use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::receipts::{Feed, SOURCE};
use super::receipts_chain::{Chain, gone};
use super::rpc;

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
#[derive(Debug)]
pub enum Fail {
    /// Повторить тот же блок через bsc.receipts_retry_ms.
    Retry(String),
    /// Ответ блока больше предела (размер/время) на всех узлах (решение 77): «жёсткий» отказ — такой блок сам по себе
    /// не станет меньше, его считаем к bsc.receipts_block_max_tries (пропуск). Повторяется как Retry.
    Over(String),
    /// Обрыв ленты.
    Break(String),
    /// Блока (или его квитанций) у узла нет (ответ null). Для блока позади головы, которого нет ни у одного узла
    /// (гонка), — пропуск невосполним (receipts_step.rs); иначе — повтор. Второе — сколько узлов ответили «блока нет»
    /// (один узел — невосполнимо только после двух «нет» подряд).
    Missing(String, usize),
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

/// Пределы запроса квитанций: срок до заголовков и тишина тела — общие (http.timeout_ms, http.body_idle_ms), общий
/// срок и размер — свои. Ошибки: Break — настройка не задана.
pub(super) fn limits(ctx: &Ctx) -> Result<Limits, Fail> {
    let base = ctx.limits().map_err(|e| Fail::Break(e.message))?;
    let set = |name: &str| ctx.int(name).map_err(|e| Fail::Break(e.message));
    Ok(Limits {
        total: Duration::from_millis(set("bsc.receipts_total_ms")?),
        max_bytes: usize::try_from(set("bsc.receipts_max_body_bytes")?).unwrap_or(usize::MAX),
        ..base
    })
}

/// Текст «квитанции больше предела». Ошибок нет.
pub(super) fn over(n: u64, f: &HttpFail, lim: &Limits) -> String {
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
