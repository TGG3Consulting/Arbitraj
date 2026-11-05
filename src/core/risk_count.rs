//! Файл: core/risk_count.rs
//! Что делает: решение 177 (Тигран, 05.11.2026) — что считается «ошибкой площадки подряд» (risk.max_errors_in_row,
//! risk.rs on_error) и кто её считает.
//! 1. Отказ ДО отправки транзакции или ордера (своя проверка или отказ площадки, деньги и газ не тронуты: расхождение с
//!    квотером и цена хуже допустимой — QuoteMismatch, нет маршрута, данные устарели, налоги не проверены, проверка пула
//!    не прошла, настройка не задана, секреты закрыты, не хватает средств, ордер отклонён (правила ордера — свои или
//!    площадки, решение 134), откат ПРОБЫ свопа — SwapReverted без факта транзакции) — не считается ни у автоматики,
//!    ни у человека. Факт транзакции в сети (GatewayError.tx) — считается всегда.
//! 2. Ручные команды человека («Продать сейчас», «Продать и перегнать» — у алерта и остановленной сделки, «Продать» в
//!    «Балансах», «Перегнать», «Перевести USDT») не считаются совсем: сбой пишется в журнал и уведомление, а паузу
//!    площадки по ошибкам ставит только автоматика. Признак «ручное» — на задаче (tokio task_local): его ставят
//!    core/commands.rs (вся команда) и места, где команда запускает свою отдельную задачу (manual.rs, manual_halted.rs,
//!    manual_usdt.rs). Продажа остатка через режим застрявшей монеты после частичной ручной продажи — снова автоматика
//!    (sell.rs, auto). Перегон выручки после «Продать и перегнать» — общий шаг закрытия (своя задача) — автоматика.
//!
//! Живой случай 05.11 (сделка 20261105131254836GAIB): нажатия «Продать и перегнать» при отказе по квотеру растили
//! счётчик ошибок bsc_dex (3 → 6 → 9) и ставили на паузу ВСЮ площадку DEX.
//! Кто вызывает: core/rules_apply.rs (counts, is_manual), core/commands.rs, manual.rs, manual_halted.rs, manual_usdt.rs,
//! sell.rs (manual, auto).
//! От чего зависит: gateway/errors.rs.
//! Настройки: нет.

use std::future::Future;

use crate::gateway::errors::{ErrorCode, GatewayError};

tokio::task_local! {
    /// true — задача выполняет ручную команду человека.
    static MANUAL: bool;
}

/// Считается ли ошибка в «ошибки площадки подряд» (без учёта ручного признака). Принимает: ошибку шлюза.
/// Возвращает: false — отказ до отправки (см. шапку, п. 1). Ошибок нет.
pub fn counts(e: &GatewayError) -> bool {
    if e.tx.is_some() {
        return true;
    }
    !matches!(
        e.code,
        ErrorCode::QuoteMismatch
            | ErrorCode::NoRoute
            | ErrorCode::StaleData
            | ErrorCode::CoinNotChecked
            | ErrorCode::CoinCheckFailed
            | ErrorCode::NotConfigured
            | ErrorCode::SecretsLocked
            | ErrorCode::InsufficientBalance
            | ErrorCode::OrderRejected
            // Решение 182: ордер биржи снят без исполнения (IOC/FOK) — деньги не тронуты, это движение рынка.
            | ErrorCode::OrderNotFilled
            | ErrorCode::NotApplicable
            | ErrorCode::SwapReverted
    )
}

/// Выполняет работу как ручную команду человека (п. 2 шапки). Принимает: работу. Возвращает: её итог. Ошибок нет.
pub async fn manual<F: Future>(f: F) -> F::Output {
    MANUAL.scope(true, f).await
}

/// Выполняет работу как автоматику внутри ручной команды (остаток после частичной продажи — режим застрявшей монеты).
/// Принимает: работу. Возвращает: её итог. Ошибок нет.
pub async fn auto<F: Future>(f: F) -> F::Output {
    MANUAL.scope(false, f).await
}

/// Идёт ли сейчас ручная команда человека. Вне признака (автоматика, другая задача) — false. Ошибок нет.
pub fn is_manual() -> bool {
    MANUAL.try_with(|m| *m).unwrap_or(false)
}
