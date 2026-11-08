//! Файл: venues/bsc_dex/resolve.rs
//! Что делает: решение 231 — исход транзакции кошелька «неизвестен» по «чем найти» из ошибки (gateway/pending.rs: хеш
//! и nonce транзакции; прикладывает venue.rs — pending_of). Квитанция по хешу (eth_getTransactionReceipt):
//! - нет — исход пока не установлен (None, ядро спросит ещё);
//! - есть — транзакция в блоке, её nonce занят. Очередь кошелька, остановленная ИМЕННО этой транзакцией (тот же хеш и
//!   nonce в WalletQueueState::Halted), снимается — как кнопкой «Снять очередь после ручной проверки» (wallet/deposit.rs
//!   release: nonce на «latest» должен быть занят, иначе не снимается). Дальше:
//!   - вывод (перевод из кошелька): успех (status 1) — перевод найден, ядро ведёт его обычным опросом статуса
//!     (withdrawal_status по хешу); откат (status 0) — WithdrawRejected: вывода нет;
//!   - своп (дополнение, Тигран 08.11: «восстанавливать итог полностью»): путь исполнения кладёт в «чем найти»
//!     контекст разбора (PendingRef.parse — exec/swap_ctx.rs: кошелёк, токены, decimals, роутеры нативной BNB,
//!     исполнитель, пометки и подписи, чаевые, каналы); итог по квитанции строят те же функции, что обычный путь
//!     (exec/swap_rebuild.rs), решение 222 (баланс до/после) — venue.rs. Разрешение (approve) в блоке — обмен не
//!     отправлялся: SwapReverted с фактом разрешения. Контекста нет (операции до дополнения, KyberSwap и «лучший из двух»
//!     с маршрутом KyberSwap, бандл, цикл DEX ↔ DEX) — NotApplicable: исход свопа выясняет человек (очередь кошелька при
//!     этом снята, если квитанция есть).
//!
//! Nonce занят, а квитанции своей транзакции нет (её место заняла отмена или чужая транзакция) — бот не решает: это
//! тот же случай «Replaced», что уже дал OutcomeUnknown (exec/result.rs); ждём квитанцию до срока ядра, дальше человек.
//!
//! Кто вызывает: venue.rs (Venue::resolve_trade, Venue::resolve_withdraw) — ядро core/outcome_resolve.rs.
//! От чего зависит: node/rpc.rs, send/receipt.rs, wallet/deposit.rs (release), wallet/transfer.rs (transfer), parts.rs,
//! exec/swap_ctx.rs, exec/swap_rebuild.rs.
//! Настройки: нет (узел — bsc.node_http_url).
//!
//! Квитанция — https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/transaction.yaml#L127-L165
//! (status: 1 — успех, 0 — откат); null, пока транзакции нет в блоке. eth_getTransactionCount «latest» — execution-apis
//! 16caee27 src/eth/state.yaml.

use alloy_primitives::B256;
use serde_json::json;

use crate::core::types::{
    ChainTx, TradeRequest, TradeResult, Transfer, TransferStatus, WalletQueueState, WithdrawRequest,
};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::pending::PendingRef;

use super::exec::swap_ctx::SwapCtx;
use super::exec::swap_rebuild;
use super::node::rpc;
use super::parts::Parts;
use super::send::receipt::Receipt;
use super::wallet;

/// «Чем найти» по факту транзакции из ошибки: хеш и nonce. Ошибок нет.
pub fn pending_of(tx: &ChainTx) -> PendingRef {
    PendingRef {
        venue_ref: Some(tx.hash.to_string()),
        nonce: Some(tx.nonce),
        ..PendingRef::default()
    }
}

/// Прикладывает «чем найти» к ошибке «исход неизвестен», если у неё есть факт транзакции. Ошибок нет.
pub fn attach(e: GatewayError) -> GatewayError {
    match e.tx.as_deref().map(pending_of) {
        Some(p) => e.pending(p),
        None => e,
    }
}

/// Что делает: своп с неизвестным исходом — квитанция есть: очередь кошелька снята (если остановлена этой
/// транзакцией), итог свопа — по контексту разбора (exec/swap_rebuild.rs; решение 222 — venue.rs). Принимает: части
/// адаптера, исходный запрос, «чем найти». Возвращает: Some(итог, как у обычного пути); None — квитанции пока нет.
/// Ошибки: NotApplicable — квитанция есть, контекста разбора нет (текст: статус и что с очередью); SwapReverted —
/// откат обмена или разрешение без обмена; OutcomeUnknown — суммы не числа (ядро спросит ещё); узел; нет хеша.
pub async fn trade(
    p: &Parts,
    req: TradeRequest,
    pend: &PendingRef,
) -> GatewayResult<Option<TradeResult>> {
    let (hash, nonce) = keys(p, pend)?;
    let Some(r) = receipt(p, hash).await? else {
        return Ok(None);
    };
    let queue = release_if_ours(p, hash, nonce).await;
    match SwapCtx::of(pend).filter(SwapCtx::complete) {
        Some(c) => swap_rebuild::rebuild(&p.ctx, req, &c, hash, r).map(Some),
        None => Err(p.ctx.err(
            ErrorCode::NotApplicable,
            format!(
                "квитанция {hash} найдена (блок {}, статус {}); {queue}; контекста разбора свопа в «чем найти» нет \
                 (KyberSwap, бандл или операция до дополнения решения 231) — сделку проверяет человек",
                r.block_number, r.status
            ),
        )),
    }
}

/// Что делает: вывод (перевод из кошелька) с неизвестным исходом. Принимает: части адаптера, исходный запрос, «чем
/// найти». Возвращает: Some(перевод с хешем, Pending — дальше опрос статуса); None — квитанции пока нет.
/// Ошибки: WithdrawRejected — перевод откатился в блоке; узел; нет хеша (BadResponse).
pub async fn withdraw(
    p: &Parts,
    req: WithdrawRequest,
    pend: &PendingRef,
) -> GatewayResult<Option<Transfer>> {
    let (hash, nonce) = keys(p, pend)?;
    let Some(r) = receipt(p, hash).await? else {
        return Ok(None);
    };
    let queue = release_if_ours(p, hash, nonce).await;
    if r.status.to::<u64>() != 1 {
        return Err(p.ctx.err(
            ErrorCode::WithdrawRejected,
            format!(
                "Перевод откатился в блоке {}, tx {hash}; {queue}",
                r.block_number
            ),
        ));
    }
    let t = wallet::transfer::transfer(
        &p.ctx,
        req.asset,
        req.network,
        req.amount,
        TransferStatus::Pending,
        hash,
        None,
    );
    Ok(Some(t))
}

/// Хеш и nonce из «чем найти». Ошибка BadResponse — нет или не 32 байта.
fn keys(p: &Parts, pend: &PendingRef) -> GatewayResult<(B256, Option<u64>)> {
    let hash = pend
        .venue_ref
        .as_deref()
        .and_then(|h| h.parse::<B256>().ok())
        .ok_or_else(|| {
            p.ctx.err(
                ErrorCode::BadResponse,
                format!("нет хеша транзакции: {pend:?}"),
            )
        })?;
    Ok((hash, pend.nonce))
}

/// Квитанция по хешу. None — транзакции нет в блоке. Ошибки: узел.
async fn receipt(p: &Parts, hash: B256) -> GatewayResult<Option<Receipt>> {
    rpc::call(&p.ctx, "eth_getTransactionReceipt", json!([hash])).await
}

/// Снимает очередь кошелька, если она остановлена этой транзакцией (тот же хеш и nonce). Возвращает: строку для
/// журнала (снята / не остановлена / остановлена другой транзакцией / не снята — причина). Ошибок нет.
async fn release_if_ours(p: &Parts, hash: B256, nonce: Option<u64>) -> String {
    p.nonces.restore(&p.ctx).await;
    match p.nonces.state() {
        WalletQueueState::Halted {
            hash: Some(h),
            nonce: n,
            ..
        } if h == hash && (nonce.is_none() || n == nonce) => {
            match wallet::deposit::release(p, "ботом по квитанции, решение 231", false).await
            {
                Ok(_) => "очередь кошелька снята".to_string(),
                Err(e) => format!("очередь кошелька не снята: {}", e.message),
            }
        }
        WalletQueueState::Halted { .. } => {
            "очередь кошелька остановлена другой транзакцией — её снимает человек".into()
        }
        _ => "очередь кошелька не остановлена".to_string(),
    }
}
