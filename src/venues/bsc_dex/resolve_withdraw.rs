//! Файл: venues/bsc_dex/resolve_withdraw.rs
//! Что делает: решение 231 — позднее выяснение своим узлом ВЫВОДА из кошелька DEX (перевод монеты на адрес депозита,
//! wallet/transfer.rs) с неизвестным исходом по «чем найти» из ошибки.
//! - Хеши отмен известны (exec/swap_ctx.rs cancels — кладут exec/txs.rs при Pending / прерванном трекере и
//!   wallet/transfer.rs при «вытеснена»; ветка t49/withdraw-cancel, как у свопа и цикла — resolve_single.rs) — вердикт
//!   resolve_single.rs verdict (голова и nonce кошелька, квитанции исходной и отмен):
//!   - исходный перевод в блоке → как ниже (landed): успех — вывод есть, дальше обычный опрос статуса; откат — вывода нет;
//!   - отмена в блоке → вывода нет: TxNotIncluded с фактом транзакции (cancel_hash, cancel_gas_bnb — газ отмены;
//!     exec/single_rebuild.rs withdraw_cancelled; решение 236, дополнение п. 4 — тот же код, что обычный путь); ядро —
//!     окончательный итог (core/outcome_resolve.rs definitive), повтор безопасен (OpError::withdraw_unsent);
//!   - nonce занят ни нашим переводом, ни отменой (или обе в блоке) → NotApplicable, исход проверяет человек;
//!   - ждём → None, ядро спросит ещё.
//! - Хешей отмен нет («чем найти» до этого поля) — как раньше: только квитанция по хешу (нет — ждём; дальше человек).
//!
//! Очередь кошелька, остановленная ИМЕННО этим переводом (тот же хеш и nonce), снимается, когда исход выяснен (resolve.rs
//! release_if_ours: nonce должен быть занят).
//! Найденный перевод несёт газ своей квитанции (fee — TransferGas, как wallet/transfer.rs; решение 236, дополнение п. 7).
//! Кто вызывает: venue.rs (Venue::resolve_withdraw) — ядро core/outcome_resolve.rs (из core/transfers.rs).
//! От чего зависит: resolve.rs (keys, receipt, release_if_ours), resolve_single.rs (verdict), exec/single_rebuild.rs,
//! exec/swap_ctx.rs, wallet/transfer.rs (transfer), wallet/balances.rs (BNB), send/receipt.rs (газ), parts.rs.
//! Настройки: через resolve_single.rs — send.replaced_confirm_blocks, feed.stale_after_ms (узел — bsc.node_http_url).
//!
//! Квитанция — https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/transaction.yaml#L127-L165
//! (status: 1 — успех, 0 — откат; null, пока транзакции нет в блоке).

use alloy_primitives::B256;

use crate::core::types::{Cost, CostKind, Transfer, TransferStatus, WithdrawRequest};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::gateway::pending::PendingRef;

use super::exec::single_rebuild::{self, Verdict};
use super::exec::swap_ctx::SwapCtx;
use super::parts::Parts;
use super::resolve::{keys, receipt, release_if_ours};
use super::send::receipt::Receipt;
use super::wallet;
use super::wallet::balances::BNB;

/// Что делает: вывод (перевод из кошелька) с неизвестным исходом (см. шапку). Принимает: части адаптера, исходный
/// запрос, «чем найти». Возвращает: Some(перевод с хешем, Pending — дальше опрос статуса); None — исход пока не установлен.
/// Ошибки: WithdrawRejected — перевод откатился в блоке; TxNotIncluded — nonce занят нашей отменой в блоке (вывода нет,
/// решение 236, дополнение п. 4);
/// NotApplicable — противоречие или нет nonce (человек); Internal — нет факта транзакции в контексте; узел, StaleData,
/// NotConfigured, ключ (ядро спросит ещё); нет хеша (BadResponse).
pub async fn withdraw(
    p: &Parts,
    req: WithdrawRequest,
    pend: &PendingRef,
) -> GatewayResult<Option<Transfer>> {
    let (hash, nonce) = keys(p, pend)?;
    if let Some(c) = SwapCtx::of(pend).filter(|c| c.cancels.is_some()) {
        let (n, v) = super::resolve_single::verdict(p, &c, (hash, nonce)).await?;
        return match v {
            Verdict::Wait(_) => Ok(None),
            Verdict::Ours(r) => landed(p, req, (hash, Some(n)), r).await.map(Some),
            Verdict::Cancelled(h, r) => {
                let queue = release_if_ours(p, hash, Some(n), false).await;
                Err(single_rebuild::withdraw_cancelled(
                    &p.ctx,
                    &c,
                    hash,
                    (h, r),
                    &queue,
                ))
            }
            Verdict::Odd(why) => {
                let queue = release_if_ours(p, hash, Some(n), false).await;
                Err(p.ctx.err(
                    ErrorCode::NotApplicable,
                    format!("перевод {hash}: {why}; {queue}; исход проверяет человек"),
                ))
            }
        };
    }
    let Some(r) = receipt(p, hash).await? else {
        return Ok(None);
    };
    landed(p, req, (hash, nonce), r).await.map(Some)
}

/// Что делает: исходный перевод в блоке (квитанция найдена): очередь кошелька снята (если остановлена этим переводом);
/// успех — перевод с хешем (Pending, дальше обычный опрос статуса) и газом квитанции (fee, как обычный путь), откат —
/// вывода нет. Принимает: части адаптера,
/// запрос, (хеш, nonce), квитанцию. Возвращает: перевод. Ошибки: WithdrawRejected — откат в блоке.
async fn landed(
    p: &Parts,
    req: WithdrawRequest,
    (hash, nonce): (B256, Option<u64>),
    r: Receipt,
) -> GatewayResult<Transfer> {
    let queue = release_if_ours(p, hash, nonce, false).await;
    if r.status.to::<u64>() != 1 {
        return Err(p.ctx.err(
            ErrorCode::WithdrawRejected,
            format!(
                "Перевод откатился в блоке {}, tx {hash}; {queue}",
                r.block_number
            ),
        ));
    }
    // Решение 236, дополнение п. 7 (техлид 09.11 по поручению Тиграна): газ перевода = gasUsed × effectiveGasPrice
    // этой квитанции, как у обычного пути (wallet/transfer.rs). В учёт идёт один раз: обычно ядро (core/transfers.rs go)
    // ждёт статус withdrawal_status (тот же газ той же квитанции) и берёт его fee, а этот перевод — только на пути
    // «зачислено по блокчейну» (core/transfer_chain.rs: вывод как есть; получатель — DEX, core/credit_chain.rs arm).
    let fee = fee(&r);
    Ok(wallet::transfer::transfer(
        &p.ctx,
        req.asset,
        req.network,
        req.amount,
        TransferStatus::Pending,
        hash,
        fee,
    ))
}

/// Что делает: газ перевода по его квитанции — затрата TransferGas в BNB (gasUsed × effectiveGasPrice), как у обычного
/// пути (wallet/transfer.rs) и статуса (wallet/transfer_status.rs). Принимает: квитанцию. Возвращает: None — газ не
/// помещается в Decimal. Ошибок нет.
pub fn fee(r: &Receipt) -> Option<Cost> {
    r.gas_bnb()
        .map(|g| Cost::new(CostKind::TransferGas, g, BNB, false))
}
