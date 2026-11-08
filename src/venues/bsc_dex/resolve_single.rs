//! Файл: venues/bsc_dex/resolve_single.rs
//! Что делает: позднее выяснение своим узлом ОДИНОЧНОЙ транзакции кошелька с неизвестным исходом (своп, разрешение,
//! цикл DEX ↔ DEX через исполнитель), когда в «чем найти» известны хеши отправленных трекером отмен (exec/swap_ctx.rs
//! cancels; ветка t42/small-fixes, дополнение решений 231 и 233 п. 4: «место могла занять отмена с газом, а её хеша в
//! "чем найти" нет»). Порядок чтения — как у бандла (resolve_bundle.rs): сначала голова и nonce кошелька на блоке
//! «голова − send.replaced_confirm_blocks», ПОТОМ квитанции исходной и всех отмен (eth_getTransactionReceipt), а если
//! квитанций нет — знает ли узел их в блоке (eth_getTransactionByHash). Вердикт — exec/single_rebuild.rs judge:
//! - исходная в блоке — итог как раньше (resolve.rs landed → exec/swap_rebuild.rs);
//! - отмена в блоке — «не исполнено»: TxNotIncluded с фактом транзакции (cancel_hash, cancel_gas_bnb — газ отмены в
//!   затраты), как обычный путь (exec/result.rs); ядро принимает его как окончательный итог (core/outcome_resolve.rs
//!   definitive: отмена в блоке), продажа — безопасный повтор (решение 229 cancel_confirmed), цикл — Closed, газ в затраты;
//! - противоречие (nonce занят ни нашей транзакцией, ни отменой; или обе в блоке) — NotApplicable, человек;
//! - ждём — None, ядро спросит ещё.
//!
//! Очередь кошелька, остановленная этой транзакцией (тот же хеш и nonce), снимается при выясненном исходе (resolve.rs
//! release_if_ours: nonce должен быть занят).
//! Кто вызывает: resolve.rs (trade — у контекста есть cancels); через него resolve_cycle.rs.
//! От чего зависит: exec/{single_rebuild, swap_ctx}.rs, node/{rpc, heads}.rs, send/receipt.rs, wallet/signer.rs (адрес
//! кошелька, если в контексте его нет — разрешение), parts.rs, resolve.rs (landed, release_if_ours).
//! Настройки: send.replaced_confirm_blocks, feed.stale_after_ms (свежая голова).
//!
//! eth_getTransactionCount(address, block) — execution-apis 16caee27 src/eth/state.yaml; квитанция —
//! src/eth/transaction.yaml#L127-L165 (null, пока нет в блоке); eth_getTransactionByHash — src/eth/transaction.yaml.

use alloy_primitives::{B256, U64};
use serde_json::json;

use crate::core::types::{TradeRequest, TradeResult};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::gateway::pending::PendingRef;

use super::exec::single_rebuild::{self, Seen, Verdict};
use super::exec::swap_ctx::SwapCtx;
use super::node::rpc;
use super::parts::Parts;
use super::send::receipt::{self, Receipt};
use super::wallet::signer;

/// Что делает: исход одиночной транзакции с известными отменами (см. шапку). Принимает: части адаптера, исходный
/// запрос, «чем найти», его контекст (cancels = Some), (хеш, nonce из «чем найти»). Возвращает: Some(итог) — исходная
/// в блоке; None — исход пока не установлен. Ошибки: TxNotIncluded — nonce занят нашей отменой в блоке (окончательно,
/// с фактом транзакции и газом отмены); NotApplicable — противоречие или нет nonce (человек); как у resolve.rs landed;
/// узел, StaleData, NotConfigured, ключ (ядро спросит ещё).
pub async fn trade(
    p: &Parts,
    req: TradeRequest,
    pend: &PendingRef,
    c: &SwapCtx,
    (hash, nonce): (B256, Option<u64>),
) -> GatewayResult<Option<TradeResult>> {
    let ctx = &p.ctx;
    let cancels = c.cancels.clone().unwrap_or_default();
    let Some(n) = nonce.or(c.chain.as_ref().map(|t| t.nonce)) else {
        return Err(ctx.err(
            ErrorCode::NotApplicable,
            format!("в «чем найти» транзакции {hash} нет nonce — исход проверяет человек"),
        ));
    };
    let wallet = match c.wallet {
        Some(w) => w,
        None => signer::address(ctx)?,
    };
    let confirm = ctx.int("send.replaced_confirm_blocks")?;
    let head = p.heads.fresh(ctx)?.number;
    let deep_tag = format!("{:#x}", head.saturating_sub(confirm));
    let deep: U64 = rpc::call(ctx, "eth_getTransactionCount", json!([wallet, deep_tag])).await?;
    let ours: Option<Receipt> = rpc::call(ctx, "eth_getTransactionReceipt", json!([hash])).await?;
    let cancel = match ours {
        Some(_) => None,
        None => receipt::find(ctx, &cancels)
            .await?
            .map(|(i, r)| (cancels[i], r)),
    };
    let mut all = vec![hash];
    all.extend_from_slice(&cancels);
    let in_block = ours.is_none() && cancel.is_none() && receipt::in_block(ctx, &all).await?;
    let seen = Seen {
        ours,
        cancel,
        in_block,
        nonce: n,
        nonce_deep: deep.to(),
        head,
        confirm,
    };
    match single_rebuild::judge(seen, cancels.len()) {
        Verdict::Wait(_) => Ok(None),
        Verdict::Ours(r) => super::resolve::landed(p, req, pend, (hash, Some(n)), r)
            .await
            .map(Some),
        Verdict::Cancelled(h, r) => {
            let queue = super::resolve::release_if_ours(p, hash, Some(n), false).await;
            single_rebuild::cancelled(ctx, c, hash, (h, r), &queue).map(Some)
        }
        Verdict::Odd(why) => {
            let queue = super::resolve::release_if_ours(p, hash, Some(n), false).await;
            Err(ctx.err(
                ErrorCode::NotApplicable,
                format!("транзакция {hash}: {why}; {queue}; исход проверяет человек"),
            ))
        }
    }
}
