//! Файл: venues/bsc_dex/send/seq_opts.rs
//! Что делает: решение 437 (1) — настройки отправки канала «Секвенсор Arbitrum» (вид Single::Sequencer), читаются в
//! send/ready.rs prepare до отправки (решение 398): окно условной отправки (channel.cond_time_window_s, решение 436),
//! метод отправки (channel.submit_method: raw — eth_sendRawTransaction, sync — eth_sendRawTransactionSync с
//! timeoutMs = channel.sync_timeout_ms) и таймаут HTTP запроса отправки (channel.http_timeout_ms — до заголовков и весь
//! обмен; ответ секвенсора ждёт постановки в блок, предел ожидания секвенсора — queueTimeout × 2, nitro
//! execution/gethexec/sequencer.go:628-656: меньший таймаут оставляет исход неизвестным). Разбор ошибок sync.
//! Кто вызывает: send/ready.rs (prepare, тело запроса, разбор ошибки), send/check.rs (показ).
//! От чего зависит: send/sequencer_cond.rs (окно), ctx.rs, settings (область «канал»); варианты channel.submit_method —
//! settings/catalog/send.rs SUBMIT_METHODS (тест сверяет).
//! Настройки: channel.http_timeout_ms (обязательна у канала sequencer), channel.submit_method,
//! channel.sync_timeout_ms (обязательна при sync), channel.cond_time_window_s [канал sequencer].
//!
//! eth_sendRawTransactionSync — EIP-7966; формат по исходнику (клон OffchainLabs/go-ethereum 260bc5dc — «nitro-geth»):
//! - internal/ethapi/api.go:1934 SendRawTransactionSync(input hexutil.Bytes, timeoutMs *hexutil.Uint64) — параметры
//!   [raw, timeoutMs] (hexutil.Uint64 — строка «0x…»); ответ — квитанция (MarshalReceipt, :2009-2030) после
//!   включения в блок; timeoutMs > tx-sync-max-timeout — урезается до него (:1962-1971);
//! - срок вышел: ошибка с кодом 4 (errors.go:125 errCodeTxSyncTimeout, :187-189), data — хеш транзакции (:1983-1992):
//!   «added to the transaction pool but wasn't processed» — исход неизвестен (транзакция могла попасть в блок позже);
//! - узел Nitro: флаги execution.rpc.tx-sync-default-timeout 20s / tx-sync-max-timeout 1m (arbitrum-docs
//!   run-a-node/nitro/cli-flags-reference.mdx:207-208; nitro-geth arbitrum/config.go:47-49, 94-95);
//! - эндпоинт секвенсора Arbitrum One принимает только eth_sendRawTransaction и eth_sendRawTransactionConditional
//!   (arbitrum-docs how-arbitrum-works/deep-dives/transaction-lifecycle.mdx:79-82) — sync шлётся на свой узел Nitro
//!   (channel.url), который пересылает транзакцию секвенсору; иначе ответ −32601 (NotConfigured с причиной);
//! - уведомление docs notices/transaction-submission-changes-notice.mdx:14-16, 33-34, 49-51: с Nitro 3.14.x (ещё не
//!   вышла) eth_sendRawTransaction отвечает при приёме в очередь без ошибок постановки в блок — «use
//!   eth_sendRawTransactionSync» (квитанция будет с blockHash = 0x00…0; бот квитанцию из ответа не берёт — итог
//!   только по своему узлу, send/track_l1.rs).

use alloy_primitives::Bytes;
use serde_json::{Value, json};

use crate::gateway::errors::{ErrorCode, GatewayError};
use crate::settings::scope::Scope;

use super::super::ctx::Ctx;
use super::sequencer_cond;

/// Метод отправки (eth_sendRawTransactionSync — EIP-7966).
pub const SYNC_METHOD: &str = "eth_sendRawTransactionSync";

/// Как канал отправляет.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Submit {
    /// eth_sendRawTransaction (или условная при окне — send/sequencer_cond.rs).
    Raw,
    /// eth_sendRawTransactionSync с timeoutMs (миллисекунды).
    Sync(u64),
}

/// Настройки отправки канала секвенсора.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeqOpts {
    /// Окно условной отправки, секунды (решение 436); None — без условия.
    pub cond: Option<u64>,
    pub submit: Submit,
    /// Таймаут HTTP запроса отправки, мс.
    pub http_ms: u64,
}

/// Читает настройки канала. Принимает: окружение, область канала. Возвращает: SeqOpts.
/// Ошибки: NotConfigured — нет channel.http_timeout_ms; channel.submit_method не из списка; sync без
/// channel.sync_timeout_ms или срок sync не меньше таймаута HTTP; sync вместе с окном условной отправки (это два
/// разных метода — один запрос не может быть обоими); Internal хранилища.
pub fn read(ctx: &Ctx, s: &Scope) -> Result<SeqOpts, GatewayError> {
    let st = &ctx.settings;
    let cond = sequencer_cond::window(ctx, s)?;
    let http_ms = st.integer_in("channel.http_timeout_ms", s)?;
    let method = match st.text_in("channel.submit_method", s) {
        Ok(m) => Some(m),
        Err(e) if e.code == ErrorCode::NotConfigured => None,
        Err(e) => return Err(e),
    };
    let submit = match method.as_deref() {
        None | Some("raw") => Submit::Raw,
        Some("sync") => Submit::Sync(st.integer_in("channel.sync_timeout_ms", s)?),
        Some(m) => {
            let why = format!("channel.submit_method = «{m}»: нет такого метода (raw или sync)");
            return Err(ctx.not_set("channel.submit_method", why));
        }
    };
    if let Submit::Sync(t) = submit {
        if cond.is_some() {
            let why = "channel.submit_method = sync и channel.cond_time_window_s заданы вместе: \
                       eth_sendRawTransactionSync и eth_sendRawTransactionConditional — \
                       разные методы, оставьте один";
            return Err(ctx.not_set("channel.submit_method", why));
        }
        if t >= http_ms {
            let why = format!(
                "channel.sync_timeout_ms ({t}) не меньше channel.http_timeout_ms ({http_ms}): \
                 HTTP оборвётся раньше ответа узла — исход неизвестен"
            );
            return Err(ctx.not_set("channel.sync_timeout_ms", why));
        }
    }
    Ok(SeqOpts {
        cond,
        submit,
        http_ms,
    })
}

/// Параметры eth_sendRawTransactionSync: [raw, timeoutMs «0x…»]. Ошибок нет.
pub fn sync_params(raw: &Bytes, timeout_ms: u64) -> Value {
    json!([raw, format!("{timeout_ms:#x}")])
}

/// Поясняет ошибку eth_sendRawTransactionSync по коду JSON-RPC ответа. Принимает: сырой ответ, ошибку разбора.
/// Возвращает: код 4 — Timeout (исход неизвестен: транзакция могла попасть в блок, send/mod.rs classify — «без ясного
/// ответа», дальше трекер по времени); −32601 — NotConfigured (узел метод не принимает). Остальное — как есть.
/// Ошибок нет.
pub fn explain_sync(text: &str, mut e: GatewayError) -> GatewayError {
    let code = serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|v| v.pointer("/error/code").and_then(Value::as_i64));
    let why = match code {
        Some(4) => {
            e.code = ErrorCode::Timeout;
            "eth_sendRawTransactionSync: транзакция принята, но квитанции нет за \
             channel.sync_timeout_ms (nitro-geth internal/ethapi/api.go:1983-1992) — \
             исход выясняет трекер своим узлом"
        }
        Some(-32601) => {
            e.code = ErrorCode::NotConfigured;
            "узел не принимает eth_sendRawTransactionSync (секвенсор Arbitrum One — только \
             eth_sendRawTransaction и eth_sendRawTransactionConditional, docs \
             transaction-lifecycle) — channel.url на свой узел Nitro или \
             channel.submit_method = raw"
        }
        _ => return e,
    };
    e.message = format!("{} — {why}", e.message);
    e
}

#[cfg(test)]
mod tests {
    use crate::settings::catalog::send::SUBMIT_METHODS;

    /// Варианты каталога — ровно те, что разбирает read (raw, sync).
    #[test]
    fn catalog_and_code_agree() {
        assert_eq!(SUBMIT_METHODS, &["raw", "sync"]);
    }
}
