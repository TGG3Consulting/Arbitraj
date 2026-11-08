//! Файл: core/withdraw_refused.rs
//! Что делает: решение 227 (дополнение, Тигран) — метка в сделке «вывод с площадки X отклонён». Живой случай: перегон
//! монеты с Bitget на DEX → Bitget отказал в выводе (WithdrawRejected, адресная книга) → продажа на месте частично →
//! остаток снова в застрявшую монету → «Перегон» снова выбирал вывод с Bitget → заведомый отказ. Теперь: вывод сделки
//! (перевод сделки или перегон) отказан площадкой с кодом WithdrawRejected / InvalidAddress / WithdrawLocked —
//! площадка-источник записывается в сделку (Deal.withdraw_refused, serde default); «Перегон» (relay.rs) перегона,
//! требующего вывода с этой площадки, не выбирает — сразу продажа на месте по порогу (relay_here.rs).
//! Кто вызывает: relay.rs (отказ вывода перегона и проверка перед перегоном), executor.rs (отказ вывода сделки).
//! От чего зависит: deal.rs, op_error.rs, gateway/errors.rs (коды), trade_ops.rs (вид площадки: у DEX метки нет).
//! Настройки: нет.

use crate::core::types::VenueId;
use crate::gateway::contract::VenueKind;
use crate::gateway::errors::ErrorCode;

use super::deal::Deal;
use super::engine::Core;
use super::op_error::OpError;

/// Отказ вывода, после которого вывод с этой площадки в сделке больше не пробуем: WithdrawRejected, InvalidAddress,
/// WithdrawLocked (повторы WithdrawLocked уже сделаны в transfers.rs, решение 166). Принимает: код ошибки. Ошибок нет.
pub fn refusal(code: Option<ErrorCode>) -> bool {
    matches!(
        code,
        Some(ErrorCode::WithdrawRejected | ErrorCode::InvalidAddress | ErrorCode::WithdrawLocked)
    )
}

/// Записывает площадку-источник в сделку, если вывод отказан кодом из refusal. Принимает: сделку, площадку вывода,
/// ошибку вывода. Возвращает: true — метка поставлена (новая или уже была). Ошибок нет.
pub fn mark(deal: &mut Deal, from: &VenueId, e: &OpError) -> bool {
    if !refusal(e.code) {
        return false;
    }
    if !deal.withdraw_refused.contains(from) {
        deal.withdraw_refused.push(from.clone());
        deal.note(format!(
            "Вывод с {} отклонён площадкой ({e}) — перегон с неё в этой сделке больше не выбирается (решение 227)",
            from.0
        ));
    }
    true
}

/// Ревью 08.11: как mark, но не для DEX — откат перевода из кошелька (газ, баланс) приходит тем же кодом
/// WithdrawRejected, а это разовый сбой, не отказ площадки: перегон с DEX в сделке не запрещается. Вид площадки не
/// известен — метка ставится (как у биржи). Принимает: ядро, сделку, площадку вывода, ошибку. Возвращает: как mark.
/// Ошибок нет.
pub fn mark_at(core: &Core, deal: &mut Deal, from: &VenueId, e: &OpError) -> bool {
    if super::trade_ops::kind(core, from).ok() == Some(VenueKind::Dex) {
        return false;
    }
    mark(deal, from, e)
}

/// Отказывала ли площадка в выводе в этой сделке. Принимает: сделку, площадку. Ошибок нет.
pub fn refused(deal: &Deal, v: &VenueId) -> bool {
    deal.withdraw_refused.contains(v)
}
