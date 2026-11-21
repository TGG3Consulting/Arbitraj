//! Файл: venues/bsc_dex/resolve_bundle.rs
//! Что делает: решение 233 — позднее выяснение своим узлом бандла свопов с неизвестным исходом (решение 231 для
//! бандла). «Чем найти» — контекст разбора с полем bundle (exec/swap_ctx.rs, exec/bundle_ref.rs: хеши всех транзакций,
//! первый nonce, maxBlockNumber, кошелёк, токены, decimals, роутеры нативной BNB). Порядок чтения важен: сначала голова
//! и nonce кошелька («latest» и на блоке «голова − send.replaced_confirm_blocks»), ПОТОМ квитанции всех транзакций
//! (eth_getTransactionReceipt), а если квитанций нет — знает ли узел их в блоке (eth_getTransactionByHash): nonce,
//! прочитанный раньше квитанций, не может «опередить» блок с нашей транзакцией. Вердикт — exec/bundle_rebuild.rs judge:
//! - «в блоке» — итог теми же функциями, что обычный путь бандла (bundle_rebuild::rebuild), решение 222 — venue.rs;
//! - «не исполнено» — окончательно (бандл истёк по maxBlockNumber с запасом send.replaced_confirm_blocks или первый
//!   nonce занят не нашей транзакцией): итог NotFilled — безопасный повтор (bundle_rebuild::not_executed);
//! - «противоречие» — NotApplicable: исход проверяет человек;
//! - «ждём» — None, ядро спросит ещё (срок не прошёл; узел не ответил — ошибка узла, ядро тоже спросит ещё).
//!
//! Очередь кошелька, остановленная ЭТИМ бандлом (хеш и nonce последней транзакции), снимается, когда исход выяснен
//! (resolve.rs release_if_ours): «в блоке» и «противоречие» — только если nonce занят (как кнопка); «не исполнено» —
//! и при свободном nonce (бандл уже не попадёт: следующая транзакция возьмёт тот же первый nonce, как settle_bundle
//! после штатного истечения).
//! Кто вызывает: resolve.rs (trade — у контекста есть bundle).
//! От чего зависит: exec/{bundle_rebuild, swap_ctx}.rs, node/{rpc,heads}.rs, send/receipt.rs (in_block), parts.rs,
//! resolve.rs (release_if_ours).
//! Настройки: send.replaced_confirm_blocks, feed.stale_after_ms (свежая голова).
//!
//! eth_getTransactionCount(address, block) — execution-apis 16caee27 src/eth/state.yaml; квитанция —
//! src/eth/transaction.yaml#L127-L165 (null, пока нет в блоке); eth_getTransactionByHash — src/eth/transaction.yaml.

use alloy_primitives::U64;
use serde_json::json;

use crate::core::types::{TradeRequest, TradeResult};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::exec::bundle_rebuild::{self, Seen, Verdict};
use super::exec::swap_ctx::SwapCtx;
use super::node::rpc;
use super::parts::Parts;
use super::send::receipt::{self, Receipt};

/// Что делает: исход бандла по своему узлу (см. шапку). Принимает: части адаптера, исходный запрос, контекст с bundle.
/// Возвращает: Some(итог: «в блоке» — как обычный путь; «не исполнено» — NotFilled); None — исход пока не установлен.
/// Ошибки: NotApplicable — контекст неполный или квитанции противоречат атомарности (текст: что видно и что с
/// очередью); SwapReverted — бандл откатился в блоке; OutcomeUnknown — суммы не числа; NotConfigured; StaleData; узел.
pub async fn trade(
    p: &Parts,
    req: TradeRequest,
    c: &SwapCtx,
) -> GatewayResult<Option<TradeResult>> {
    let ctx = &p.ctx;
    let (Some(b), Some(wallet)) = (c.bundle.as_ref().filter(|_| c.complete()), c.wallet) else {
        return Err(ctx.err(
            ErrorCode::NotApplicable,
            "контекст разбора бандла в «чем найти» неполный — исход бандла проверяет человек",
        ));
    };
    let confirm = ctx.int("send.replaced_confirm_blocks")?;
    let head = p.heads.fresh(ctx)?.number;
    let latest: U64 = rpc::call(ctx, "eth_getTransactionCount", json!([wallet, "latest"])).await?;
    let deep_tag = format!("{:#x}", head.saturating_sub(confirm));
    let deep: U64 = rpc::call(ctx, "eth_getTransactionCount", json!([wallet, deep_tag])).await?;
    let mut receipts = Vec::with_capacity(b.hashes.len());
    for h in &b.hashes {
        let r: Option<Receipt> = rpc::call(ctx, "eth_getTransactionReceipt", json!([h])).await?;
        receipts.push(r);
    }
    let in_block =
        receipts.iter().all(Option::is_none) && receipt::in_block(ctx, &b.hashes).await?;
    let seen = Seen {
        receipts,
        in_block,
        nonce_latest: latest.to(),
        nonce_deep: deep.to(),
        head,
        confirm,
    };
    let (hash, nonce) = (b.last_hash(), Some(b.last_nonce()));
    match bundle_rebuild::judge(b, &seen) {
        Verdict::Wait(_) => Ok(None),
        Verdict::Landed(rs) => {
            super::resolve::release_if_ours(p, hash, nonce, false).await;
            bundle_rebuild::rebuild(ctx, req, c, rs).map(Some)
        }
        Verdict::NotExecuted(why) => {
            let queue = super::resolve::release_if_ours(p, hash, nonce, true).await;
            bundle_rebuild::not_executed(ctx, req, c, &format!("{why}; {queue}")).map(Some)
        }
        Verdict::Odd(why) => {
            let queue = super::resolve::release_if_ours(p, hash, nonce, false).await;
            Err(ctx.err(
                ErrorCode::NotApplicable,
                format!("бандл {hash}: {why} — бандл атомарен, так быть не должно; {queue}; исход проверяет человек"),
            ))
        }
    }
}
