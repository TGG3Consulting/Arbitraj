//! Файл: venues/bsc_dex/send/absent.rs
//! Что делает: решение 154 — проверка своим узлом, что транзакции, которую не принял ни один канал, действительно нет
//! ни в блоке, ни в пуле узла. Только после неё отказ всех каналов (ChannelRejected) значит «точно не ушла», и ядро
//! может безопасно повторить продажу новой котировкой и новой транзакцией (core/sell_unsent.rs). Узел знает
//! транзакцию (в пуле или в блоке) или проверка не удалась — отказа нет: транзакция считается отправленной «без
//! ясного ответа» и идёт обычным путём трекера (send/track.rs: ждём её исход, при необходимости — отмена).
//! Кто вызывает: send/mod.rs (classify — когда ни один канал не принял).
//! От чего зависит: node/rpc.rs, ctx.rs, send/mod.rs (Dispatched).
//! Настройки: нет (узел — bsc.node_http_url).
//!
//! Почему так:
//! - eth_getTransactionByHash: объект транзакции или null, если узел её не знает; blockNumber = null — транзакция в
//!   пуле узла (ещё не в блоке) — https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/transaction.yaml
//!   (тот же вызов — send/receipt.rs in_block, send/track.rs).
//! - Хеш транзакции = keccak256 её подписанного кодирования EIP-2718 (для legacy — RLP) — https://eips.ethereum.org/EIPS/eip-2718 ;
//!   поэтому хеш считается прямо по отправленным байтам, без подписанта.
//! - Ответы каналов с кодом -32000 (тексты узла BSC — venues/bsc_dex/errors.rs): «already known» — канал уже знает
//!   транзакцию, это не отказ (send/ready.rs); «nonce too low» — nonce уже занят в блоке (своей транзакцией или
//!   чужой — проверка по хешу различает); «replacement transaction underpriced» — в пуле канала другая транзакция с тем
//!   же nonce; «transaction underpriced», «insufficient funds…», «execution reverted» и отказы билдеров — транзакция не
//!   принята. Вывод «точно не ушла» делается не по тексту, а по факту узла (этот файл).
//! - Пул узла — только открытый пул: закрытый канал, ответивший отказом, транзакцию не хранит; повторная продажа
//!   берёт nonce заново (wallet/nonce.rs) — пока прежний nonce свободен, обе транзакции не попадут в блок вместе.

use alloy_primitives::{B256, Bytes, keccak256};
use serde_json::{Value, json};

use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::super::node::rpc;
use super::Dispatched;

/// Итог проверки.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Узел не знает ни одну из транзакций: точно не ушли.
    Absent,
    /// Узел знает транзакцию: хеш и где она («в пуле узла» / «в блоке N»).
    Known(B256, String),
    /// Проверка не удалась (узел): причина.
    Unchecked(String),
}

/// Хеши отправленных транзакций по их байтам (keccak256 кодирования EIP-2718). Ошибок нет.
pub fn hashes(txs: &[Bytes]) -> Vec<B256> {
    txs.iter().map(keccak256).collect()
}

/// Где транзакция по ответу eth_getTransactionByHash. Принимает: result ответа (null — узел не знает).
/// Возвращает: None — не знает; Some(текст) — «в пуле узла» или «в блоке N». Ошибок нет.
pub fn located(result: &Value) -> Option<String> {
    if result.is_null() {
        return None;
    }
    Some(match result.get("blockNumber").and_then(Value::as_str) {
        Some(n) => format!("в блоке {n}"),
        None => "в пуле узла".to_string(),
    })
}

/// Спрашивает свой узел о каждой транзакции. Принимает: окружение, хеши. Возвращает: Verdict. Ошибок нет: сбой узла —
/// Verdict::Unchecked (вызывающий тогда считает транзакцию возможно отправленной).
pub async fn check(ctx: &Ctx, hashes: &[B256]) -> Verdict {
    for h in hashes {
        match rpc::call::<Value>(ctx, "eth_getTransactionByHash", json!([h])).await {
            Ok(v) => {
                if let Some(at) = located(&v) {
                    return Verdict::Known(*h, at);
                }
            }
            Err(e) => {
                return Verdict::Unchecked(format!("eth_getTransactionByHash {h}: {}", e.message));
            }
        }
    }
    Verdict::Absent
}

/// Решение 154: ни один канал не принял — свой узел подтверждает, что транзакций нет ни в блоке, ни в пуле
/// (check выше). Подтвердил — ChannelRejected «точно не ушла» (ядро может повторить продажу новой транзакцией).
/// Узел транзакцию знает или проверка не удалась — отказа нет: все каналы считаются «без ясного ответа», транзакция
/// идёт к трекеру (ждём исход, при необходимости — отмена), повтора нет. Принимает: каналы, причины, транзакции.
/// Возвращает: Dispatched (все каналы — без ясного ответа). Ошибки: ChannelRejected.
pub async fn rejected_all(
    ctx: &Ctx,
    ids: Vec<String>,
    why: Vec<String>,
    txs: &[Bytes],
) -> GatewayResult<Dispatched> {
    let doubt = match check(ctx, &hashes(txs)).await {
        Verdict::Absent => {
            return Err(ctx.err(
                ErrorCode::ChannelRejected,
                format!(
                    "Ни один канал не принял транзакцию: {} (проверено своим узлом: транзакции нет ни в блоке, ни в пуле)",
                    why.join("; ")
                ),
            ));
        }
        Verdict::Known(h, at) => format!("транзакция {h} {at}"),
        Verdict::Unchecked(e) => format!("проверка своим узлом не удалась: {e}"),
    };
    ctx.log(
        Level::Warn,
        "send.channel_rejected",
        format!(
            "Ни один канал не принял транзакцию, но {doubt} — считается отправленной, ждём исход ({})",
            why.join("; ")
        ),
        None,
    )
    .await;
    Ok(Dispatched {
        accepted: Vec::new(),
        uncertain: ids,
    })
}
