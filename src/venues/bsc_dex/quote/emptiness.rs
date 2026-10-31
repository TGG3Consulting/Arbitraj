//! Файл: venues/bsc_dex/quote/emptiness.rs
//! Что делает: пустой ли пул — по его локальному состоянию: V2 — нулевой резерв, V3 и CL — ликвидность на текущем
//! тике 0 (liquidity() / getLiquidity), Bin — в прочитанной полосе нет непустых бинов. Пустой пул — не «сбой
//! квотера»: квотер на нём откатывается (QuoterV2 — Error("Unexpected")) или считает по брошенной ликвидности далеко
//! от рынка. Такой пул не котируется и не сверяется с квотером, пока событие (Mint / ModifyLiquidity / Sync) не вернёт
//! ему ликвидность. Там же — причины исключения для сверки с квотером (check/quoter_cycle.rs, exec/precheck.rs).
//! Кто вызывает: quote/select.rs, quote/quoter.rs, exec/precheck.rs, pools/listen_log.rs (пул снова не пуст).
//! От чего зависит: pools/registry.rs, core/types (PoolState).
//! Настроек нет.
//!
//! Состояние пулов — research/flow/bsc_dex.md, раздел 3 (V2 getReserves/Sync, V3 slot0/liquidity), bsc_v4.md, раздел 4
//! (getSlot0, getLiquidity, getBin). Откат QuoterV2 на пуле с liquidity() = 0 проверен координатором eth_call к BSC
//! (PCS V3 ADA/USDT fee 100/500/10000 и др.; Issue #7).

use crate::core::types::PoolState;

use super::super::pools::registry::Entry;

/// Причина: пул пуст.
pub const EMPTY: &str = "пустой пул (ликвидность 0)";

/// Причина: у пула не хватает ликвидности на сумму сверки — один класс для локального расчёта и квотера.
pub const SHORT: &str = "у пула не хватает ликвидности на сумму сверки";

/// Уточнение: локальный расчёт не прошёл всю сумму в прочитанной карте тиков (бинов).
pub const SHORT_LOCAL: &str = "расчёт вышел за прочитанную карту тиков (бинов)";

/// Причина «не хватает ликвидности» с уточнением. Ошибок нет.
pub fn short(detail: &str) -> String {
    format!("{SHORT} ({detail})")
}

/// Пуст ли пул (см. шапку). Bin без прочитанных бинов — не пуст (не прочитан — это решает достоверность). Ошибок нет.
pub fn empty(e: &Entry) -> bool {
    match &e.pool.state {
        PoolState::V2 { reserve0, reserve1 } => reserve0.is_zero() || reserve1.is_zero(),
        PoolState::V3 { liquidity, .. }
        | PoolState::Cl { liquidity, .. }
        // Algebra (THENA Fusion, решения 80, 81): одна ликвидность (reinvestL нет) — пуст при liquidity == 0.
        | PoolState::Algebra { liquidity, .. } => *liquidity == 0,
        // Elastic: ликвидность обмена L = base_l + reinvest_l — пуст, когда оба нуля (решение 81).
        PoolState::Elastic {
            base_l, reinvest_l, ..
        } => *base_l == 0 && *reinvest_l == 0,
        PoolState::Bin { .. } => e.bins.as_ref().is_some_and(|b| b.reserves.is_empty()),
        // DODO PMM (решение 86b): пуст, когда оба резерва нули (одной стороны достаточно для своей стороны обмена —
        // односторонний DVM с Q=0 продаёт котируемую за базовую; стороны фильтрует глубина, решение 73).
        PoolState::Pmm(d) => d.b.is_zero() && d.q.is_zero(),
        // Solidly (Topaz v2, THENA V1; решения 80, 85): пуст при нулевом любом резерве (как V2 — обмен невозможен).
        PoolState::Solidly {
            reserve0, reserve1, ..
        } => reserve0.is_zero() || reserve1.is_zero(),
    }
}
