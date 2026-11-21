//! Файл: core/stuck_quote.rs
//! Что делает: решение 238 (ревью, п. 2) — ошибка котировки продажи застрявшей монеты. Код NotConfigured (не задана
//! настройка или привязка — например, venue.book_stale_ms площадки: стакана для ордера нет, exec_book.rs; дополнение,
//! Тигран 09.11: согласовано — venue.quote_asset, кошелёк счёта DEX, NotConfigured котировки DEX и REST стакана,
//! trade_ops.rs deal_quote / sell_quote) — ждать тика монеты бессмысленно, настройка сама не появится: сделка сразу
//! остановлена (Halted) с причиной «не задана настройка или привязка …»,
//! журнал Error, уведомление закрытия сделки человеку (ledger::finish). Прочие ошибки — ожидание тика (stuck.rs); первая
//! ошибка ожидания — в журнал Warn.
//! Кто вызывает: core/stuck.rs («Ожидание», «Порог убытка»), core/relay.rs («Перегон»: оценка «продать здесь»).
//! От чего зависит: op_error.rs, ledger.rs, deal.rs, deps (журнал), gateway/errors.rs.
//! Настройки: нет.

use crate::gateway::errors::ErrorCode;
use crate::storage::journal::Level;

use super::deal::{Deal, Step};
use super::engine::Core;
use super::op_error::OpError;

/// Решение 238 (ревью, п. 2): ошибка котировки — «не задана настройка» (код NotConfigured). Ошибок нет.
pub fn not_configured(e: &OpError) -> bool {
    e.code == Some(ErrorCode::NotConfigured)
}

/// Решение 238 (ревью, п. 2): котировки нет, потому что не задана настройка, — сделка остановлена (Halted) с причиной,
/// журнал Error; уведомление человеку — закрытие сделки (ledger::finish). Принимает: ядро, сделку, ошибку. Ошибок нет.
pub async fn halt_not_configured(core: &Core, mut deal: Deal, e: &OpError) {
    let text = format!(
        "застрявшая монета: котировки продажи нет — не задана настройка или привязка ({}); ждать тика бессмысленно, задайте её и решите вручную",
        e.message
    );
    core.inner
        .deps
        .log(
            Level::Error,
            "core.stuck",
            Some(&deal.at.0),
            format!("{}: {text}", deal.id),
            None,
        )
        .await;
    super::ledger::finish(core, &mut deal, Step::Halted, &text).await
}

/// Первая ошибка котировки в ожидании тика — в журнал (Warn), дальше молча (ждём тика). Принимает: ядро, сделку,
/// ошибку, флаг «ещё не писали» (сбрасывается). Ошибок нет.
pub async fn no_quote_once(core: &Core, deal: &Deal, e: &OpError, first: &mut bool) {
    if !std::mem::take(first) {
        return;
    }
    let text = format!(
        "{}: застрявшая монета — котировки продажи нет ({}), ждём тика монеты",
        deal.id, e.message
    );
    core.inner
        .deps
        .log(Level::Warn, "core.stuck", Some(&deal.at.0), text, None)
        .await;
}
