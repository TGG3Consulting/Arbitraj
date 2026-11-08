//! Файл: venues/bsc_dex/exec/swap_rebuild.rs
//! Что делает: решение 231, дополнение (Тигран, 08.11) — итог свопа DEX по найденной квитанции и контексту разбора из
//! «чем найти» (exec/swap_ctx.rs) — ТЕМИ ЖЕ функциями, что обычный путь: факт транзакции — exec/chain_tx.rs (исход по
//! статусу квитанции, блок, газ, чаевые), суммы — exec/result.rs (события Transfer, нативная BNB по роутерам) или
//! executor/fact.rs (возврат исполнителя), пометка «по квотеру» (решение 177), подпись исполнителя (exec/result.rs by),
//! причина «лучший из двух» (как best_of/trade.rs tag). Решение 222 (количество по балансу кошелька) — venue.rs, как у
//! обычного пути. Разрешение (approve) в блоке — обмен после него не отправлялся: SwapReverted с фактом разрешения
//! (газ потрачен, ничего не обменяно) — как обычный путь отвечает на непрошедшее разрешение (exec/swap_send.rs).
//! Чистая логика, без сети: квитанцию читает resolve.rs.
//! Кто вызывает: resolve.rs (trade), тесты (tests/bsc_dex_swap_resolve.rs).
//! От чего зависит: exec/{swap_ctx, chain_tx, result, txs}.rs, executor/fact.rs, send/track_util.rs, core/types.
//! Настройки: нет.
//!
//! Квитанция: status 1 — успех, 0 — откат — https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/transaction.yaml#L127-L165

use alloy_primitives::{B256, U64};

use crate::core::types::{TradeRequest, TradeResult, TxOutcome};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::send::receipt::Receipt;
use super::super::send::track_util::Tracked;
use super::executor::fact;
use super::swap_ctx::SwapCtx;
use super::{chain_tx, result, txs};

/// Что делает: итог свопа (или разрешения) по квитанции своей транзакции. Принимает: окружение (тексты ошибок), исходный
/// запрос, контекст (complete), хеш транзакции, квитанцию. Возвращает: TradeResult — как обычный путь до решения 222.
/// Ошибки: Internal — контекст неполный (вызывающий проверяет complete); SwapReverted — откат обмена или разрешение без
/// обмена (с фактом транзакции); OutcomeUnknown — суммы не переводятся в числа (как обычный путь).
pub fn rebuild(
    ctx: &Ctx,
    req: TradeRequest,
    c: &SwapCtx,
    hash: B256,
    r: Receipt,
) -> GatewayResult<TradeResult> {
    let Some(tmpl) = c.chain.as_ref() else {
        return Err(ctx.err(ErrorCode::Internal, "контекст разбора без факта транзакции"));
    };
    let outcome = if r.status == U64::from(1u8) {
        TxOutcome::Included
    } else {
        TxOutcome::Reverted
    };
    let t = Tracked {
        outcome,
        receipt: Some(r.clone()),
        cancel: None,
        unresolved: None,
    };
    let chain = chain_tx::chain_tx(
        hash,
        tmpl.nonce,
        tmpl.channels.clone(),
        tmpl.method,
        &t,
        c.tip_bnb,
    );
    let signed = |x: GatewayResult<TradeResult>| match &c.who {
        Some(w) => result::by(x, w),
        None => x,
    };
    if c.approve {
        let why = format!(
            "Разрешение (approve) tx {hash} в блоке {} ({outcome:?}) — обмен после него не отправлялся, ничего не обменяно",
            r.block_number
        );
        return signed(Err(ctx.err(ErrorCode::SwapReverted, why).with_tx(chain)));
    }
    let (Some(wallet), Some(tokens), Some(decs)) = (c.wallet, c.tokens, c.decs) else {
        return Err(ctx.err(ErrorCode::Internal, "контекст разбора свопа неполный"));
    };
    let d = txs::Done {
        chain,
        receipt: Some(r),
        cancel: None,
        tip_bnb: c.tip_bnb,
        cancels: Vec::new(),
    };
    let out = match c.executor {
        Some(x) => fact::result(ctx, req, d, (wallet, x), tokens, decs),
        None => result::result(ctx, req, d, wallet, tokens, decs, &c.native_routers),
    };
    // Решение 177: пометка «по квотеру» — как в пути (trade_direct.rs, executor/trade.rs: r.executor = note).
    let out = out.map(|mut t| {
        if c.note.is_some() {
            t.executor = c.note.clone();
        }
        t
    });
    tail(signed(out), c.best_of.as_deref())
}

/// Причина «лучший из двух» — как best_of/trade.rs tag. Принимает: итог, причину (None — без изменений). Ошибок нет.
pub fn tail(r: GatewayResult<TradeResult>, why: Option<&str>) -> GatewayResult<TradeResult> {
    let Some(why) = why else {
        return r;
    };
    match r {
        Ok(mut t) => {
            let who = t.executor.take().unwrap_or_default();
            t.executor = Some(format!("{who}; лучший из двух: {why}"));
            Ok(t)
        }
        Err(mut e) => {
            e.message = format!("{} (лучший из двух: {why})", e.message);
            Err(e)
        }
    }
}
