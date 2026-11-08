//! Файл: venues/bsc_dex/resolve_cycle.rs
//! Что делает: решение 233 (дополнение, п. 4; Тигран 08.11: «ок», «делать сейчас») — позднее выяснение своим узлом
//! сделки цикла DEX ↔ DEX с неизвестным исходом (Venue::resolve_cycle). Цикл — своп «базовая → базовая», поэтому
//! исход ищется ТЕМ ЖЕ механизмом, что у свопа (решения 231 / 233, resolve.rs trade): одиночная транзакция через
//! исполнитель — квитанция по хешу (нет — ждём; есть — итог exec/swap_rebuild.rs → exec/result.rs, как обычный путь
//! цикла exec/cycle_exec.rs); бандл — resolve_bundle.rs (голова и nonce кошелька, затем квитанции всех транзакций:
//! «в блоке» — итог exec/bundle_rebuild.rs → bundle_trade::bundle_result, как exec/cycle_bundle_trade.rs; «окончательно
//! не исполнено» — срок бандла прошёл с запасом send.replaced_confirm_blocks или первый nonce занят не нашей транзакцией:
//! NotFilled без газа; «противоречие» — человек). Очередь кошелька, остановленная этой транзакцией (у бандла —
//! последней), снимается там же (resolve.rs release_if_ours), когда исход выяснен. Затем итог свопа → итог цикла с
//! курсами сделки из «чем найти» (exec/cycle_ref.rs finish). Разрешение (approve) исполнителю в блоке — SwapReverted
//! «обмен не отправлялся» (как у свопа). Одиночная: nonce занят, а своей квитанции нет — бот не решает (как resolve.rs:
//! место могла занять отмена с газом, её хеша в «чем найти» нет) — ждём до срока ядра, дальше человек.
//! Кто вызывает: venue.rs (Venue::resolve_cycle) — ядро core/cycle_outcome.rs.
//! От чего зависит: resolve.rs (trade), exec/{cycle_ref, cycle_steps, swap_ctx}.rs, parts.rs.
//! Настройки: нет своих (через resolve_bundle.rs — send.replaced_confirm_blocks, feed.stale_after_ms).

use crate::core::types::{CycleTradeRequest, CycleTradeResult};
use crate::gateway::errors::GatewayResult;
use crate::gateway::pending::PendingRef;

use super::exec::swap_ctx::SwapCtx;
use super::exec::{cycle_ref, cycle_steps};
use super::parts::Parts;

/// Что делает: исход цикла по «чем найти» (см. шапку). Принимает: части адаптера (кошелёк сделки), исходный запрос
/// цикла, «чем найти». Возвращает: Some(итог цикла — как обычный путь; «не исполнено» — нули, NotFilled, транзакции не в
/// блоке); None — исход пока не установлен (ядро спросит ещё). Ошибки: SwapReverted — цикл (или разрешение) откатился в
/// блоке, с фактом транзакции; NotApplicable — контекст неполный / нет курсов цикла / квитанции противоречат
/// атомарности бандла (человек); OutcomeUnknown — суммы не числа; узел, StaleData, NotConfigured (ядро спросит ещё).
pub async fn cycle(
    p: &Parts,
    req: CycleTradeRequest,
    pend: &PendingRef,
) -> GatewayResult<Option<CycleTradeResult>> {
    let c = SwapCtx::of(pend);
    let tr = cycle_steps::trade_request(&req);
    match super::resolve::trade(p, tr, pend).await? {
        None => Ok(None),
        Some(t) => cycle_ref::finish(&p.ctx, t, c.as_ref()).map(Some),
    }
}
