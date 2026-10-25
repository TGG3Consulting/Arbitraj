//! Файл: gateway/rules.rs
//! Что делает: модель правил шлюза и выбор правила для ошибки.
//! Правило = событие (код ошибки) + необязательное условие + действие + область.
//! Правило одной площадки сильнее общего правила.
//! Кто вызывает: шлюз при каждой ошибке; api/rules.rs (интерфейс правил).
//! От чего зависит: gateway/errors, core/types (VenueId).
//! Настройки: сами правила хранятся в SQLite (storage/rules.rs), меняются из интерфейса.
//!
//! Готовых правил в коде нет: все правила, включая стартовое правило Тиграна
//! («ордер не исполнился — отменяем, больше не пробуем»), вводятся в интерфейсе.
//! Формат условия будет утверждён на этапе 5 (docs/ROADMAP.md); пока условие хранится как JSON.

use serde::{Deserialize, Serialize};

use crate::core::types::VenueId;

use super::errors::ErrorCode;

/// Область действия правила.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "venue")]
pub enum RuleScope {
    /// На все площадки.
    All,
    /// Только на одну площадку — сильнее общего.
    Venue(VenueId),
}

/// Действие правила.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuleAction {
    /// Отменить операцию и больше не пробовать.
    CancelNoRetry,
    /// Повторить операцию (число повторов и пауза — в параметрах правила).
    Retry { attempts: u32, pause_ms: u64 },
    /// Остановить торговлю по площадке.
    StopVenue,
    /// Остановить всю торговлю.
    StopAll,
    /// Только уведомить.
    NotifyOnly,
}

/// Правило шлюза.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    /// Идентификатор в хранилище.
    pub id: i64,
    /// На какую ошибку срабатывает.
    pub event: ErrorCode,
    /// Дополнительное условие (формат — на этапе 5); None — без условия.
    pub condition: Option<serde_json::Value>,
    pub action: RuleAction,
    pub scope: RuleScope,
    pub enabled: bool,
}

/// Выбирает правило для ошибки: сначала правило площадки, затем общее.
/// Принимает: все правила, код ошибки, площадку.
/// Возвращает: подходящее включённое правило или None (правила нет — шлюз
/// останавливает операцию и уведомляет, см. gateway/README.md).
/// Условия на этапе 0 не проверяются: правило с условием не выбирается.
pub fn pick_rule<'a>(
    rules: &'a [Rule],
    code: ErrorCode,
    venue: Option<&VenueId>,
) -> Option<&'a Rule> {
    let usable = |r: &&Rule| r.enabled && r.event == code && r.condition.is_none();
    if let Some(v) = venue {
        let specific = rules
            .iter()
            .filter(usable)
            .find(|r| r.scope == RuleScope::Venue(v.clone()));
        if specific.is_some() {
            return specific;
        }
    }
    rules
        .iter()
        .filter(usable)
        .find(|r| r.scope == RuleScope::All)
}

/// Можно ли повторять операцию по этому коду. «Результат неизвестен» (OutcomeUnknown: ордер, вывод или транзакция
/// отправлены, итог не получен) — никогда, какие бы правила ни были заданы: стартовое правило Тиграна «ордер отправлен
/// и не исполнился — отменяем, больше не пробуем» (CLAUDE.md, решение 64). Вместо повтора — остановка сделки
/// до ручной проверки. Принимает: код. Ошибок нет.
pub fn retry_allowed(code: ErrorCode) -> bool {
    code != ErrorCode::OutcomeUnknown
}
