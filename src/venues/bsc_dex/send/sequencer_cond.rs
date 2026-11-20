//! Файл: venues/bsc_dex/send/sequencer_cond.rs
//! Что делает: решение 436 (А) — условная отправка в секвенсор Arbitrum: eth_sendRawTransactionConditional(raw,
//! options). Условие одно — окно времени L2: timestampMax = время головы своего узла на момент отправки + окно
//! channel.cond_time_window_s (секунды, настройка канала). Секвенсор проверяет условие, когда ставит транзакцию в
//! блок; блок позже окна — транзакция не принимается (ошибка −32003) и в сеть не попадает: план сделки по устаревшим
//! ценам не исполнится, если очередь секвенсора задержала транзакцию дольше окна. Окно не задано — обычная
//! eth_sendRawTransaction (как до решения 436). Отмена (send/cancel.rs) — всегда без условия: её дело занять nonce.
//! Окна блока L1 (blockNumberMin/Max) и knownAccounts не ставятся — docs/TASKS.md, «Решение 436».
//! Кто вызывает: send/ready.rs (окно канала, тело запроса, разбор ошибки), exec/txs.rs (head_time до отправки).
//! От чего зависит: node/heads.rs, node/head_age.rs (время головы), send/ready.rs, ctx.rs.
//! Настройки: channel.cond_time_window_s [канал sequencer].
//!
//! Источники (клоны: OffchainLabs/go-ethereum 260bc5dcbd5a16925d7e26145577b2612f30f428 — «nitro-geth»;
//! OffchainLabs/nitro ccc5c828d9ea22f161efc784bac4d4a6559b7e59; OffchainLabs/arbitrum-docs 5ce1a9e1c75f…):
//! - метод: nitro-geth arbitrum/conditionaltx.go:25 SendRawTransactionConditional(input hexutil.Bytes, options
//!   *ConditionalOptions) в пространстве «eth» (arbitrum/apibackend.go:177-182) → eth_sendRawTransactionConditional;
//!   ответ — хеш транзакции (conditionaltx.go:60); так же шлёт сам Nitro (conditionaltx.go:63-68);
//! - параметры: nitro-geth arbitrum_types/txoptions.go:71-77 — knownAccounts (адрес → корень хранилища или {слот:
//!   значение}), blockNumberMin/Max (номер блока L1), timestampMin/Max (время блока L2), числа HexOrDecimal64:
//!   «0x…» или десятичные (common/math/integer.go:32-52, при записи — "%#x"); пустой knownAccounts — допустим
//!   (nitro system_tests/conditionaltx_test.go:88-89 — выполнимые условия);
//! - проверка: txoptions.go:79-107 (TimestampMax: время блока L2 > timestampMax → «TimestampMax condition not met»);
//!   вызывается секвенсором при постановке в блок с header.Time строящегося блока (nitro
//!   execution/gethexec/sequencer.go:789-805) и проверкой перед пересылкой (tx_pre_checker.go:183-189); ответ
//!   ждёт итога постановки (sequencer.go:614-651, resultChan);
//! - ошибки: −32003 «rejected» — условие не выполнено (txoptions.go:15-23, 79-107); −32005 «limit exceeded»
//!   (txoptions.go:25-33, только через WrapOptionsCheckError :35-47); код ошибки JSON-RPC — rpc/json.go:124-138;
//! - docs.arbitrum.io «RPC methods» (content/docs/arbitrum-essentials/arbitrum-vs-ethereum/rpc-methods.mdx:46-58);
//!   уведомление: метод отключают на секвенсоре Arbitrum One 17.01.2027, после — −32601 method not found
//!   (content/docs/notices/eth-send-raw-transaction-conditional-deprecation-notice.mdx:12, 26-30).

use alloy_primitives::Bytes;
use serde_json::{Value, json};

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::settings::scope::Scope;

use super::super::ctx::Ctx;
use super::super::node::head_age;
use super::super::node::heads::Heads;
use super::ready::Ready;

/// Метод условной отправки (nitro-geth arbitrum/conditionaltx.go:25, пространство «eth»).
pub const METHOD: &str = "eth_sendRawTransactionConditional";

/// Какое условие ставить.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cond {
    /// Без условия — обычная eth_sendRawTransaction.
    Plain,
    /// «Проверить подключение»: условный метод с пустыми условиями (пустая транзакция — канал ответит ошибкой).
    Probe,
    /// timestampMax — время блока L2 (секунды Unix) не позже этого.
    Until(u64),
}

/// Окно условия канала: channel.cond_time_window_s. Принимает: окружение, область канала.
/// Возвращает: Some(секунды) или None — не задана (условия нет). Ошибки: Internal хранилища.
pub fn window(ctx: &Ctx, s: &Scope) -> Result<Option<u64>, GatewayError> {
    match ctx.settings.integer_in("channel.cond_time_window_s", s) {
        Ok(w) => Ok(Some(w)),
        Err(e) if e.code == ErrorCode::NotConfigured => Ok(None),
        Err(e) => Err(e),
    }
}

/// Время головы для условия — только если у какого-то из выбранных каналов задано окно (иначе узел не спрашивается).
/// Читается ДО отправки (решение 398). Ревью 436: время именно блока головы — Heads::at, пока время головы ещё тянет
/// другой запрос, отдаёт время прошлого блока (решение 89, для расчёта); для условия оно дало бы раннее timestampMax
/// и лишние отказы −32003, поэтому тогда время головы читается с узла (node/head_age.rs fetch) и запоминается.
/// Принимает: окружение, головы, каналы. Возвращает: секунды Unix или None.
/// Ошибки: как Heads::at (нет свежего блока — StaleData; узел); BadResponse (узел не знает блок).
pub async fn head_time(ctx: &Ctx, heads: &Heads, ready: &[Ready]) -> GatewayResult<Option<u64>> {
    if ready.iter().all(|r| r.cond.is_none()) {
        return Ok(None);
    }
    let head = heads.fresh(ctx)?.number;
    let (n, t) = heads.at(ctx).await?;
    if n == head {
        return Ok(Some(t));
    }
    let bt = head_age::fetch(ctx, head, true).await?;
    heads.remember_time(head, bt);
    Ok(Some(bt.secs))
}

/// Параметры условной отправки: [raw, options]. Принимает: подписанную транзакцию, условие (Probe или Until).
/// Возвращает: массив параметров JSON-RPC. Ошибок нет.
pub fn params(raw: &Bytes, cond: Cond) -> Value {
    match cond {
        Cond::Until(t) => json!([raw, { "knownAccounts": {}, "timestampMax": format!("{t:#x}") }]),
        Cond::Plain | Cond::Probe => json!([raw, { "knownAccounts": {} }]),
    }
}

/// Поясняет ошибку условной отправки по коду JSON-RPC ответа. Принимает: сырой ответ, ошибку разбора.
/// Возвращает: ошибку с пояснением: −32003 и −32005 — отказ секвенсора (ChannelRejected, транзакция не принята);
/// −32601 — метод не принимается: NotConfigured (не «Подключено» в проверке), снять окно у канала. Ошибок нет.
pub fn explain(text: &str, mut e: GatewayError) -> GatewayError {
    let code = serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|v| v.pointer("/error/code").and_then(Value::as_i64));
    let why = match code {
        Some(-32003) => {
            "условие eth_sendRawTransactionConditional не выполнено — секвенсор транзакцию не принял \
             (nitro-geth arbitrum_types/txoptions.go:79-107)"
        }
        Some(-32005) => "предел условий превышен (nitro-geth arbitrum_types/txoptions.go:25-33)",
        Some(-32601) => {
            e.code = ErrorCode::NotConfigured;
            "секвенсор не принимает eth_sendRawTransactionConditional (docs.arbitrum.io: метод отключают \
             17.01.2027) — снимите channel.cond_time_window_s у канала, отправка станет обычной"
        }
        _ => return e,
    };
    e.message = format!("{} — {why}", e.message);
    e
}
