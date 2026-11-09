//! Файл: venues/bsc_dex/resolve.rs
//! Что делает: решение 231 — исход транзакции кошелька «неизвестен» по «чем найти» из ошибки (gateway/pending.rs: хеш
//! и nonce транзакции; прикладывает venue.rs — pending_of). Квитанция по хешу (eth_getTransactionReceipt):
//! - нет — исход пока не установлен (None, ядро спросит ещё);
//! - есть — транзакция в блоке, её nonce занят. Очередь кошелька, остановленная ИМЕННО этой транзакцией (тот же хеш и
//!   nonce в WalletQueueState::Halted), снимается — как кнопкой «Снять очередь после ручной проверки» (wallet/deposit.rs
//!   release: nonce на «latest» должен быть занят, иначе не снимается). Дальше:
//!   - вывод (перевод из кошелька) — resolve_withdraw.rs: успех (status 1) — перевод найден, ядро ведёт его обычным
//!     опросом статуса (withdrawal_status по хешу); откат (status 0) — WithdrawRejected: вывода нет; хеши отмен известны
//!     — как у свопа ниже (отмена в блоке — вывода нет; ветка t49/withdraw-cancel);
//!   - своп (дополнение, Тигран 08.11: «восстанавливать итог полностью»): путь исполнения кладёт в «чем найти»
//!     контекст разбора (PendingRef.parse — exec/swap_ctx.rs: кошелёк, токены, decimals, роутеры нативной BNB,
//!     исполнитель, пометки и подписи, чаевые, каналы); итог по квитанции строят те же функции, что обычный путь
//!     (exec/swap_rebuild.rs), решение 222 (баланс до/после) — venue.rs. Разрешение (approve) в блоке — обмен не
//!     отправлялся: SwapReverted с фактом разрешения. Бандл (решение 233) — resolve_bundle.rs: квитанции всех транзакций
//!     и nonce кошелька. Контекста нет (операции до дополнения, KyberSwap и «лучший из двух»
//!     с маршрутом KyberSwap) — NotApplicable: исход свопа выясняет человек (очередь кошелька при этом снята, если
//!     квитанция есть). Цикл DEX ↔ DEX (решение 233, дополнение, п. 4) — своп «базовая → базовая» с контекстом цикла:
//!     resolve_cycle.rs зовёт этот же trade и делает из итога свопа итог цикла.
//!
//! Хеши отправленных отмен есть в «чем найти» (exec/swap_ctx.rs cancels; ветка t42/small-fixes) — resolve_single.rs:
//! квитанция отмены в блоке → «не исполнено» (TxNotIncluded, газ отмены — в факте транзакции); nonce занят ни нашей
//! транзакцией, ни отменой → противоречие, человек. Хешей отмен нет («чем найти» до этого поля) — как раньше: nonce занят,
//! а квитанции своей транзакции нет — бот не решает, ждёт квитанцию до срока ядра, дальше человек.
//!
//! Кто вызывает: venue.rs (Venue::resolve_trade) — ядро core/outcome_resolve.rs; resolve_bundle.rs, resolve_single.rs
//! (release_if_ours, landed); resolve_cycle.rs (trade — цикл DEX ↔ DEX); resolve_withdraw.rs (keys, receipt,
//! release_if_ours — вывод).
//! От чего зависит: node/rpc.rs, send/receipt.rs, wallet/deposit.rs (release), parts.rs,
//! exec/swap_ctx.rs, exec/swap_rebuild.rs, exec/chain_tx.rs (газ перевода чаевых найденного итога — решение 236,
//! дополнение п. 6), resolve_bundle.rs, resolve_single.rs.
//! Настройки: нет (узел — bsc.node_http_url).
//!
//! Квитанция — https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/transaction.yaml#L127-L165
//! (status: 1 — успех, 0 — откат); null, пока транзакции нет в блоке. eth_getTransactionCount «latest» — execution-apis
//! 16caee27 src/eth/state.yaml.

use alloy_primitives::B256;
use serde_json::json;

use crate::core::types::{ChainTx, TradeRequest, TradeResult, WalletQueueState};
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
    // Решение 233: бандл — квитанции ВСЕХ его транзакций и nonce кошелька (resolve_bundle.rs), не одна по хешу.
    if let Some(c) = SwapCtx::of(pend).filter(|c| c.bundle.is_some()) {
        return super::resolve_bundle::trade(p, req, &c).await;
    }
    let (hash, nonce) = keys(p, pend)?;
    // Хеши отмен известны (ветка t42): квитанции отмен и nonce кошелька — resolve_single.rs.
    if let Some(c) = SwapCtx::of(pend).filter(|c| c.cancels.is_some()) {
        return super::resolve_single::trade(p, req, pend, &c, (hash, nonce)).await;
    }
    let Some(r) = receipt(p, hash).await? else {
        return Ok(None);
    };
    landed(p, req, pend, (hash, nonce), r).await.map(Some)
}

/// Что делает: своп в блоке (квитанция исходной транзакции найдена): очередь кошелька снята (если остановлена этой
/// транзакцией), итог по контексту разбора. Принимает: части адаптера, запрос, «чем найти», (хеш, nonce), квитанцию.
/// Возвращает: итог, как у обычного пути. Ошибки: как у trade (NotApplicable — контекста нет; SwapReverted; OutcomeUnknown).
pub(super) async fn landed(
    p: &Parts,
    req: TradeRequest,
    pend: &PendingRef,
    (hash, nonce): (B256, Option<u64>),
    r: Receipt,
) -> GatewayResult<TradeResult> {
    let queue = release_if_ours(p, hash, nonce, false).await;
    match SwapCtx::of(pend).filter(SwapCtx::complete) {
        Some(c) => {
            let out = swap_rebuild::rebuild(&p.ctx, req, &c, hash, r);
            tip_gas(p, c.chain.as_ref().and_then(|t| t.tip_hash), out).await
        }
        None => Err(p.ctx.err(
            ErrorCode::NotApplicable,
            format!(
                "квитанция {hash} найдена (блок {}, статус {}); {queue}; контекста разбора свопа в «чем найти» нет \
                 (KyberSwap или операция до дополнения решения 231) — сделку проверяет человек",
                r.block_number, r.status
            ),
        )),
    }
}

/// Что делает: решение 236, дополнение п. 6 — газ перевода чаевых у итога, найденного по квитанции, как у обычного
/// пути (exec/txs.rs → exec/chain_tx.rs tip_gas): хеш перевода — из факта в «чем найти», газ — по его квитанции, если
/// чаевые в блоке (в блоке / откат). Принимает: части адаптера, хеш перевода чаевых (None — не было или «чем найти» до
/// этого поля), итог (успех — TradeResult.chain, ошибка — GatewayError.tx). Возвращает: тот же итог. Ошибок своих нет.
async fn tip_gas(
    p: &Parts,
    tip_hash: Option<B256>,
    out: GatewayResult<TradeResult>,
) -> GatewayResult<TradeResult> {
    if tip_hash.is_none() {
        return out;
    }
    let fix = |c: ChainTx| async move {
        let mut c = c;
        c.tip_hash = tip_hash;
        super::exec::chain_tx::tip_gas(&p.ctx, c).await
    };
    match out {
        Ok(mut t) => {
            if let Some(c) = t.chain.take() {
                t.chain = Some(fix(c).await);
            }
            Ok(t)
        }
        Err(mut e) => {
            if let Some(c) = e.tx.take() {
                e.tx = Some(Box::new(fix(*c).await));
            }
            Err(e)
        }
    }
}

/// Хеш и nonce из «чем найти». Ошибка BadResponse — нет или не 32 байта.
pub(super) fn keys(p: &Parts, pend: &PendingRef) -> GatewayResult<(B256, Option<u64>)> {
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
pub(super) async fn receipt(p: &Parts, hash: B256) -> GatewayResult<Option<Receipt>> {
    rpc::call(&p.ctx, "eth_getTransactionReceipt", json!([hash])).await
}

/// Снимает очередь кошелька, если она остановлена этой транзакцией (тот же хеш и nonce; у бандла — последней его
/// транзакцией, resolve_bundle.rs). Принимает: хеш, nonce, pending_ok — снимать и при свободном nonce (только бандл,
/// исход «не исполнено»: он уже не попадёт в блок). Возвращает: строку для журнала (снята / не остановлена /
/// остановлена другой транзакцией / не снята — причина). Ошибок нет.
pub(super) async fn release_if_ours(
    p: &Parts,
    hash: B256,
    nonce: Option<u64>,
    pending_ok: bool,
) -> String {
    p.nonces.restore(&p.ctx).await;
    match p.nonces.state() {
        WalletQueueState::Halted {
            hash: Some(h),
            nonce: n,
            ..
        } if h == hash && (nonce.is_none() || n == nonce) => {
            match wallet::deposit::release(p, "ботом по квитанции, решение 231", pending_ok).await
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
