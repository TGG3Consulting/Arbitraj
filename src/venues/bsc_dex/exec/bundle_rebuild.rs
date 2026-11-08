//! Файл: venues/bsc_dex/exec/bundle_rebuild.rs
//! Что делает: решение 233 — позднее выяснение бандла свопов с неизвестным исходом, чистая логика без сети (сеть —
//! resolve_bundle.rs). (1) judge: по тому, что видит свой узел (квитанции ВСЕХ транзакций бандла, знает ли узел их в
//! блоке, nonce кошелька, голова), — вердикт:
//! - все квитанции есть, в одном блоке, статусы одинаковые → «в блоке» (итог — rebuild);
//! - квитанций нет, узел их в блоке не знает, и голова дальше maxBlockNumber + send.replaced_confirm_blocks (каналы
//!   бандл после предела не включают, запас — на отставание индекса узла и короткую реорганизацию) ИЛИ первый nonce
//!   бандла занят не нашей транзакцией не позже блока «голова − send.replaced_confirm_blocks» → «не исполнено»
//!   окончательно: бандл в блок уже не попадёт, отказ ДО исполнения, безопасный повтор;
//! - часть квитанций, разные блоки или разные статусы → «противоречие»: бандл атомарен (revertingTxHashes пуст), так
//!   быть не должно — выясняет человек (NotApplicable);
//! - иначе (срок не прошёл, узел знает транзакцию в блоке без квитанции) — «ждём», ядро спросит ещё.
//!
//! (2) rebuild: «в блоке» → итог ТЕМИ ЖЕ функциями, что обычный путь бандла (exec/bundle_trade.rs bundle_result →
//! exec/result.rs, нативная BNB по роутерам бандла), подпись исполнителя (exec/result.rs by) и причина «лучший из
//! двух» (exec/swap_rebuild.rs tail); решение 222 (баланс до/после) — venue.rs, как у trade.
//!
//! (3) not_executed: «не исполнено» → TradeResult NotFilled (0 монет, газа нет — бандл не в блоке): ядро разбирает его
//! как «ордер не исполнился» (core/trade_ops.rs → OrderNotFilled; продажа — повтор режимом застрявшей монеты, 182).
//!
//! Кто вызывает: resolve_bundle.rs; тесты (tests/bsc_dex_bundle_resolve.rs).
//! От чего зависит: exec/{bundle_ref, bundle_send, bundle_trade, result, swap_ctx, swap_rebuild}.rs, send/{receipt,
//! track_util}.rs, core/types.
//! Настройки: нет (send.replaced_confirm_blocks читает resolve_bundle.rs и передаёт в Seen).
//!
//! Квитанция: status 1 — успех, 0 — откат; null, пока нет в блоке —
//! https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/transaction.yaml#L127-L165

use alloy_primitives::U64;
use chrono::Utc;
use rust_decimal::Decimal;

use crate::core::types::{ChainTx, SendMethod, TradeRequest, TradeResult, TradeStatus, TxOutcome};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::send::receipt::Receipt;
use super::super::send::track_util::Tracked;
use super::bundle_ref::BundleRef;
use super::bundle_send::Sent;
use super::swap_ctx::SwapCtx;
use super::{bundle_trade, result, swap_rebuild};

/// Что видит свой узел по бандлу.
pub struct Seen {
    /// Квитанции по хешам бандла, в том же порядке (None — нет в блоке).
    pub receipts: Vec<Option<Receipt>>,
    /// Узел знает хотя бы одну транзакцию бандла в блоке (eth_getTransactionByHash), а квитанции ещё нет.
    pub in_block: bool,
    /// eth_getTransactionCount(кошелёк, «latest»).
    pub nonce_latest: u64,
    /// eth_getTransactionCount(кошелёк, голова − confirm) — прочитан ДО квитанций.
    pub nonce_deep: u64,
    /// Номер свежей головы.
    pub head: u64,
    /// send.replaced_confirm_blocks.
    pub confirm: u64,
}

/// Вердикт по бандлу.
#[derive(Debug)]
pub enum Verdict {
    /// Исход пока не установлен (причина — для журнала).
    Wait(String),
    /// Все транзакции бандла в блоке — квитанции по порядку.
    Landed(Vec<Receipt>),
    /// Бандл не исполнен и уже не исполнится (причина).
    NotExecuted(String),
    /// Квитанции противоречат атомарности бандла — человек (причина).
    Odd(String),
}

/// Что делает: вердикт по бандлу (см. шапку). Принимает: «чем найти» бандла, что видит узел. Возвращает: вердикт.
/// Ошибок нет.
pub fn judge(b: &BundleRef, s: &Seen) -> Verdict {
    let n = b.hashes.len();
    let found: Vec<&Receipt> = s.receipts.iter().flatten().collect();
    if s.receipts.len() != n || n == 0 {
        return Verdict::Odd(format!(
            "квитанций прочитано {} на {n} хешей бандла",
            s.receipts.len()
        ));
    }
    if found.len() == n {
        let (b0, st0) = (found[0].block_number, found[0].status);
        if found.iter().any(|r| r.block_number != b0) {
            return Verdict::Odd("транзакции бандла в разных блоках".into());
        }
        if found.iter().any(|r| r.status != st0) {
            return Verdict::Odd(format!(
                "в блоке {b0} часть транзакций бандла откатилась, часть нет"
            ));
        }
        return Verdict::Landed(found.into_iter().cloned().collect());
    }
    if !found.is_empty() {
        return Verdict::Odd(format!("в блоке {} из {n} транзакций бандла", found.len()));
    }
    if s.in_block {
        return Verdict::Wait("узел знает транзакцию бандла в блоке, квитанции ещё нет".into());
    }
    let state = if s.nonce_latest > b.first_nonce {
        format!("nonce {} занят не транзакцией бандла", b.first_nonce)
    } else {
        format!("nonce {} свободен", b.first_nonce)
    };
    if s.head > b.max_block + s.confirm {
        return Verdict::NotExecuted(format!(
            "квитанций нет, блок {} дальше maxBlockNumber {} + send.replaced_confirm_blocks ({}) — бандл истёк; {state}",
            s.head, b.max_block, s.confirm
        ));
    }
    if s.nonce_deep > b.first_nonce {
        return Verdict::NotExecuted(format!(
            "квитанций нет, nonce {} занят не транзакцией бандла не позже блока {}",
            b.first_nonce,
            s.head.saturating_sub(s.confirm)
        ));
    }
    Verdict::Wait(format!(
        "квитанций нет, блок {} не дальше maxBlockNumber {} + send.replaced_confirm_blocks ({}); {state}",
        s.head, b.max_block, s.confirm
    ))
}

/// Что делает: итог бандла «в блоке» по квитанциям всех транзакций — как обычный путь (exec/bundle_trade.rs
/// bundle_result: исход по статусу квитанции первой транзакции, как confirm в exec/bundle_send.rs). Принимает:
/// окружение, исходный запрос, контекст (complete, bundle), квитанции по порядку. Возвращает: TradeResult до решения
/// 222. Ошибки: Internal — контекст неполный; SwapReverted — откат (факт бандла); OutcomeUnknown — суммы не числа.
pub fn rebuild(
    ctx: &Ctx,
    req: TradeRequest,
    c: &SwapCtx,
    receipts: Vec<Receipt>,
) -> GatewayResult<TradeResult> {
    let (Some(b), Some(tmpl), Some(wallet), Some(tokens), Some(decs)) = (
        c.bundle.as_ref(),
        c.chain.as_ref(),
        c.wallet,
        c.tokens,
        c.decs,
    ) else {
        return Err(ctx.err(ErrorCode::Internal, "контекст разбора бандла неполный"));
    };
    let Some(first) = receipts.first().cloned() else {
        return Err(ctx.err(ErrorCode::Internal, "квитанций бандла нет"));
    };
    let outcome = if first.status == U64::from(1u8) {
        TxOutcome::Included
    } else {
        TxOutcome::Reverted
    };
    let sent = Sent {
        hashes: b.hashes.clone(),
        channels: tmpl.channels.clone(),
        tracked: Tracked {
            outcome,
            receipt: Some(first),
            cancel: None,
            unresolved: None,
        },
        receipts,
        max_block: b.max_block,
    };
    let nr = (b.first_nonce, c.native_routers.as_slice());
    let out = bundle_trade::bundle_result(ctx, req, &sent, wallet, tokens, decs, nr);
    finish(out, c)
}

/// Что делает: бандл не исполнен и уже не исполнится — итог «не исполнено» (0 монет, газа нет), причина — в
/// пометке исполнения (хронология сделки). Принимает: окружение, исходный запрос, контекст, причину. Возвращает:
/// TradeResult NotFilled с фактом бандла (исход NotIncluded). Ошибки: Internal — контекст неполный.
pub fn not_executed(
    ctx: &Ctx,
    req: TradeRequest,
    c: &SwapCtx,
    why: &str,
) -> GatewayResult<TradeResult> {
    let (Some(b), Some(tmpl)) = (c.bundle.as_ref(), c.chain.as_ref()) else {
        return Err(ctx.err(ErrorCode::Internal, "контекст разбора бандла неполный"));
    };
    let chain = ChainTx {
        hash: b.last_hash(),
        nonce: b.first_nonce,
        channels: tmpl.channels.clone(),
        method: SendMethod::Bundle,
        outcome: TxOutcome::NotIncluded,
        block: None,
        gas_used: None,
        gas_bnb: None,
        tip_bnb: None,
        cancel_hash: None,
        cancel_gas_bnb: None,
    };
    let t = TradeResult {
        status: TradeStatus::NotFilled,
        filled_qty: Decimal::ZERO,
        avg_price: Decimal::ZERO,
        fees: Vec::new(),
        venue_ref: chain.hash.to_string(),
        chain: Some(chain),
        finished_at: Utc::now(),
        executor: Some(format!("бандл не исполнен (решение 233): {why}")),
        request: req,
    };
    finish(Ok(t), c)
}

/// Подпись исполнителя и причина «лучший из двух» — как обычный путь (exec/trade.rs by, best_of/trade.rs tag).
fn finish(out: GatewayResult<TradeResult>, c: &SwapCtx) -> GatewayResult<TradeResult> {
    let out = match &c.who {
        Some(w) => result::by(out, w),
        None => out,
    };
    swap_rebuild::tail(out, c.best_of.as_deref())
}
