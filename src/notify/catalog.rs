//! Файл: notify/catalog.rs
//! Что делает: список событий, которые можно присылать в Telegram — по ТЗ,
//! раздел «Уведомления в Telegram». Каждое событие включается и настраивается в интерфейсе
//! (notify.event.enabled / venues / condition с областью «событие»).
//! Кто вызывает: notify/dispatch.rs, api/settings.rs (список для экрана «Уведомления»).
//! От чего зависит: serde.
//! Настройки: нет (настройки событий — в settings).
//!
//! Новая фича добавляет сюда свои события (CLAUDE.md, «Готовая фича»).

use serde::Serialize;

/// Описание события уведомлений.
#[derive(Debug, Clone, Serialize)]
pub struct EventDef {
    /// Код события — ключ области «событие» в настройках.
    pub code: &'static str,
    /// Группа на экране «Уведомления».
    pub group: &'static str,
    /// Что это за событие — по-русски.
    pub title: &'static str,
}

/// Короткая запись одного события. Принимает: группу, код, название. Возвращает: EventDef.
const fn e(group: &'static str, code: &'static str, title: &'static str) -> EventDef {
    EventDef { code, group, title }
}

/// Все события. Порядок — как в ТЗ.
pub static EVENTS: &[EventDef] = &[
    e("Сделки", "deal.candidate", "Кандидат прошёл расчёт"),
    e("Сделки", "deal.buy_filled", "Покупка исполнена"),
    e("Сделки", "deal.transfer_sent", "Перевод отправлен"),
    e("Сделки", "deal.transfer_credited", "Перевод зачислен"),
    e("Сделки", "deal.sell_filled", "Продажа исполнена"),
    e(
        "Сделки",
        "deal.closed",
        "Сделка закрыта: прибыль или убыток, план против факта",
    ),
    e(
        "Ошибки исполнения",
        "exec.order_not_filled",
        "Ордер не исполнился",
    ),
    e("Ошибки исполнения", "exec.swap_reverted", "Своп откатился"),
    e(
        "Ошибки исполнения",
        "exec.withdraw_rejected",
        "Вывод отклонён",
    ),
    e(
        "Ошибки исполнения",
        "exec.transfer_slow",
        "Перевод дольше нормы",
    ),
    e(
        "Ошибки исполнения",
        "exec.deposit_not_credited",
        "Депозит не зачислен",
    ),
    e("Риск", "risk.stop", "Сработало стоп-условие"),
    e("Риск", "risk.daily_loss", "Убыток за день достиг лимита"),
    e(
        "Риск",
        "risk.reconcile_mismatch",
        "Расхождение при сверке балансов",
    ),
    e("Риск", "risk.low_bnb", "Мало BNB на газ"),
    e("Риск", "risk.low_usdt", "Мало USDT для покупок"),
    e("Площадки", "venue.disconnected", "Разрыв соединения"),
    e("Площадки", "venue.reconnected", "Соединение восстановлено"),
    e("Площадки", "venue.rate_limited", "Превышен лимит запросов"),
    e(
        "Площадки",
        "venue.transfer_status",
        "У монеты закрылся или открылся ввод или вывод",
    ),
    e(
        "Монеты и направления",
        "route.quarantine_in",
        "Монета или направление ушли в карантин",
    ),
    e(
        "Монеты и направления",
        "route.quarantine_out",
        "Монета или направление вышли из карантина",
    ),
    e("Система", "system.started", "Запуск"),
    e("Система", "system.stopped", "Остановка"),
    e(
        "Система",
        "system.secrets_unlocked",
        "Секреты разблокированы",
    ),
    e(
        "Система",
        "system.settings_changed",
        "Изменение настроек — кто, что и когда поменял",
    ),
    e(
        "Сводка",
        "summary",
        "Периодическая сводка: сделки, прибыль, ошибки",
    ),
];

/// Ищет событие по коду. Возвращает описание или None.
pub fn find(code: &str) -> Option<&'static EventDef> {
    EVENTS.iter().find(|e| e.code == code)
}
