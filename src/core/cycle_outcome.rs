//! Файл: core/cycle_outcome.rs
//! Что делает: решение 233 (дополнение, п. 4; Тигран 08.11: «ок», «делать сейчас») — бот сам выясняет исход сделки
//! цикла DEX ↔ DEX, отправленной с неизвестным исходом (OutcomeUnknown: итог трекера не установлен — Pending у бандла
//! или у одиночной транзакции через исполнитель, слежение прервано, квитанции бандла прочитаны не все). Тот же механизм,
//! что у ордера / свопа (решение 231, outcome_resolve.rs poll / done): после паузы площадки (rules_stop.rs регистрирует
//! операцию «выясняет бот», если у ошибки есть «чем найти» и заданы risk.outcome_poll_s / risk.outcome_max_s) каждые
//! risk.outcome_poll_s спрашивает тот же адаптер (кошелёк сделки) — Venue::resolve_cycle, не дольше risk.outcome_max_s.
//! Найден итог — вызывающий (cycle_deal.rs) закрывает сделку: «в блоке» — как обычный исполненный цикл; «окончательно
//! не исполнено» (not_executed) — цикл не исполнен, денег не стоил; откат в блоке (SwapReverted) — как обычный откат,
//! газ в затраты; nonce занят нашей отменой в блоке (TxNotIncluded с хешем отмены) — не исполнен, газ отмены в затраты. Не выяснен / человек (NotApplicable, противоречие, срок) — как раньше: сделка Halted, пауза «только
//! человек». Очередь кошелька снимает адаптер, когда исход выяснен.
//! Выяснение идёт в задаче сделки цикла (её ждёт поиск циклов core/cycle_watch.rs; площадка всё это время на паузе
//! «исход неизвестен», очередь кошелька остановлена — новых циклов этим кошельком и так нет).
//! Кто вызывает: cycle_deal.rs.
//! От чего зависит: outcome_resolve.rs (poll, done), gateway (contract, errors, pending), core/types.
//! Настройки: risk.outcome_poll_s, risk.outcome_max_s (через outcome_resolve.rs).

use std::sync::Arc;

use crate::core::types::{CycleTradeRequest, CycleTradeResult, TradeStatus, TxOutcome, VenueId};
use crate::gateway::contract::Venue;
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::engine::Core;
use super::outcome_resolve;

/// Название операции в журнале и уведомлениях (то же, что у правила шлюза цикла — cycle_deal.rs).
pub const OP: &str = "Цикл DEX ↔ DEX";

/// Что делает: исход цикла по «чем найти» из ошибки. Принимает: ядро, сделку, площадку, адаптер (кошелёк сделки),
/// исходный запрос, ошибку. Возвращает: Some(Ok(итог)) — найден («в блоке» или «окончательно не исполнено»);
/// Some(Err(ошибка)) — найден откат в блоке (окончательно); None — не выяснен (человек). Ошибок нет.
pub async fn resolve(
    core: &Core,
    deal: &str,
    v: &VenueId,
    x: &Arc<dyn Venue>,
    req: &CycleTradeRequest,
    e: &GatewayError,
) -> Option<GatewayResult<CycleTradeResult>> {
    let p = e.pending.as_deref()?.clone();
    let found =
        outcome_resolve::poll(core, OP, deal, v, e, || x.resolve_cycle(req.clone(), &p)).await?;
    let line = match &found {
        Ok(r) if not_executed(r) => format!(
            "цикл не исполнен — транзакции не в блоке ({})",
            r.trade.executor.clone().unwrap_or_default()
        ),
        Ok(r) => format!(
            "цикл в блоке: вход {} → выход {}, tx {}",
            r.input, r.output, r.trade.venue_ref
        ),
        // Откат в блоке или nonce занят нашей отменой в блоке (TxNotIncluded, outcome_resolve.rs definitive).
        Err(x) if x.code == ErrorCode::SwapReverted => format!("откатился в блоке: {}", x.message),
        Err(x) => format!("не исполнен: {}", x.message),
    };
    outcome_resolve::done(core, OP, deal, v, e, &line).await;
    Some(found)
}

/// Что делает: «окончательно не исполнено» — итог без исполнения (NotFilled), а его транзакции не в блоке (NotIncluded:
/// бандл истёк или nonce занят не им). Принимает: найденный итог. Возвращает: true — цикл не исполнен, газа нет.
/// Ошибок нет.
pub fn not_executed(r: &CycleTradeResult) -> bool {
    r.trade.status == TradeStatus::NotFilled
        && r.trade
            .chain
            .as_ref()
            .is_some_and(|c| c.outcome == TxOutcome::NotIncluded)
}
