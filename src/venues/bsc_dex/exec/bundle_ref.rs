//! Файл: venues/bsc_dex/exec/bundle_ref.rs
//! Что делает: решение 233 (Тигран 08.11: «всегда понимать — ушла, не ушла; обрабатывать, а не валить всё в ошибках и
//! дублировать») — исход бандла свопов (решение 84) НЕ установлен → «исход неизвестен» (OutcomeUnknown), а не «не
//! попала в блок» (TxNotIncluded). Два случая: (1) трекер бандла (exec/bundle_send.rs confirm) не установил итог за
//! send.queue_max_wait_s — узел молчал, блоков не было, квитанции не читались (Pending; очередь кошелька остановлена,
//! send/settle.rs); бандл мог попасть в блок позже, и повтор продажи продал бы второй раз; (2) бандл в блоке, а
//! квитанции прочитаны не все — факт сделки по неполным логам был бы занижен. Здесь же — «чем найти» бандла
//! (BundleRef в контексте разбора exec/swap_ctx.rs): хеши ВСЕХ транзакций по порядку, первый nonce, предел блока
//! maxBlockNumber; позднее выяснение — resolve_bundle.rs → exec/bundle_rebuild.rs.
//! Кто вызывает: exec/bundle_trade.rs (bundle_result — проверка; trade — BundleRef в контекст), exec/swap_ctx.rs (поле),
//! exec/bundle_rebuild.rs, resolve_bundle.rs.
//! От чего зависит: exec/bundle_send.rs (Sent), gateway/errors.rs, core/types (ChainTx, TxOutcome).
//! Настройки: нет.

use alloy_primitives::B256;
use serde::{Deserialize, Serialize};

use crate::core::types::{ChainTx, TxOutcome};
use crate::gateway::errors::{ErrorCode, GatewayError};

use super::super::ctx::Ctx;
use super::bundle_send::Sent;

/// «Чем найти» бандл: всё, что нужно своему узлу, чтобы позже установить его исход.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleRef {
    /// Хеши транзакций бандла по порядку (nonce first_nonce, first_nonce + 1, …).
    pub hashes: Vec<B256>,
    /// Nonce первой транзакции бандла.
    pub first_nonce: u64,
    /// maxBlockNumber бандла (eth_sendBundle): после этого блока каналы его не включают.
    pub max_block: u64,
}

impl BundleRef {
    /// Что делает: «чем найти» по итогу отправки. Принимает: итог отправки, первый nonce. Ошибок нет.
    pub fn of(sent: &Sent, first_nonce: u64) -> Self {
        Self {
            hashes: sent.hashes.clone(),
            first_nonce,
            max_block: sent.max_block,
        }
    }

    /// Nonce последней транзакции бандла — им очередь кошелька остановлена (exec/bundle_send.rs). Ошибок нет.
    pub fn last_nonce(&self) -> u64 {
        self.first_nonce + (self.hashes.len() as u64).saturating_sub(1)
    }

    /// Хеш последней транзакции бандла — им очередь кошелька остановлена (exec/bundle_send.rs). Ошибок нет.
    pub fn last_hash(&self) -> B256 {
        self.hashes.last().copied().unwrap_or_default()
    }
}

/// Что делает: исход бандла не установлен — ошибка «исход неизвестен» с фактом бандла (ядро не повторяет, площадка на
/// паузу, бот выясняет по «чем найти» — exec/bundle_trade.rs прикладывает контекст). Принимает: окружение (текст),
/// итог отправки, факт бандла. Возвращает: Some(ошибка) — итог Pending / не установлен, либо «в блоке», а квитанций
/// меньше, чем транзакций; None — исход установлен (разбирает exec/result.rs). Ошибок своих нет.
pub fn unknown(ctx: &Ctx, sent: &Sent, chain: &ChainTx) -> Option<GatewayError> {
    let t = &sent.tracked;
    let n = sent.hashes.len();
    let why = if let Some(why) = &t.unresolved {
        format!("{why} — очередь кошелька остановлена до выяснения")
    } else if t.outcome == TxOutcome::Pending {
        "итог бандла не установлен".to_string()
    } else if t.outcome == TxOutcome::Included && sent.receipts.len() != n {
        format!(
            "бандл в блоке, но прочитаны квитанции {} из {n} — факт сделки по ним был бы неполным",
            sent.receipts.len()
        )
    } else {
        return None;
    };
    let hashes: Vec<String> = sent.hashes.iter().map(|h| h.to_string()).collect();
    let text = format!(
        "Исход бандла неизвестен ({why}); транзакций {n}, первый nonce {}, maxBlockNumber {}, хеши: {}",
        chain.nonce,
        sent.max_block,
        hashes.join(", ")
    );
    Some(
        ctx.err(ErrorCode::OutcomeUnknown, text)
            .with_tx(chain.clone()),
    )
}
