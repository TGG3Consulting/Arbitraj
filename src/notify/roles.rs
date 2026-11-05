//! Файл: notify/roles.rs
//! Что делает: роли чатов Telegram (решение 176, Тигран 05.11.2026) — явная модель «четыре чата»:
//! «Управление» (Control: меню кнопками — состояние, балансы, продажа, перевод USDT), «Общий» (General: прежний чат
//! notify.telegram_chat — всё как до решения), «Сделки» (Deals: только закрытые плюсовые сделки), «Неудачные»
//! (Failures: зеркало «Ручного управления» с кнопками, минусовые закрытия). У каждой роли своя пара «токен бота
//! (секрет) + ID чата (настройка)» и свой список разрешённых пользователей. Роль без ID чата не задана — её сообщения
//! не уходят и её кнопки не принимаются (General — как раньше: без чата уведомления не уходят, причина в журнале).
//! Кто вызывает: notify/client.rs (клиент роли), notify/routes.rs (куда событие), notify/updates*.rs (входящие по
//! чату → роль), notify/press.rs (разрешённые пользователи), notify/check.rs, core/commands.rs (чат команды — один
//! из чатов ролей), api/check.rs.
//! От чего зависит: settings, secrets (имена секретов), notify/tg_buttons.rs (Chat, User).
//! Настройки: notify.telegram_chat, notify.control_chat, notify.deals_chat, notify.failures_chat;
//! notify.general_users, notify.control_users, notify.failures_users; секреты telegram.bot_token,
//! telegram.control_token, telegram.deals_token, telegram.failures_token.

use crate::secrets;
use crate::settings::read::At;
use crate::settings::store::Store;

use super::tg_buttons::{Chat, User};

/// Роль чата.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Role {
    /// «Управление»: главное меню кнопками (решение 176, чат 1).
    Control,
    /// «Общий»: прежний чат уведомлений (чат 2) — без изменений поведения.
    General,
    /// «Сделки»: только закрытые сделки с итогом ≥ 0 (чат 3).
    Deals,
    /// «Неудачные»: записи «Ручного управления» с кнопками и минусовые закрытия (чат 4).
    Failures,
}

impl Role {
    /// Все роли — порядок экрана «Уведомления».
    pub const ALL: [Role; 4] = [Role::Control, Role::General, Role::Deals, Role::Failures];

    /// Код роли для адреса API (POST /api/check/telegram/{роль}). Ошибок нет.
    pub fn code(self) -> &'static str {
        match self {
            Role::Control => "control",
            Role::General => "general",
            Role::Deals => "deals",
            Role::Failures => "failures",
        }
    }

    /// Роль по коду. Возвращает: роль или None — нет такой. Ошибок нет.
    pub fn from_code(code: &str) -> Option<Role> {
        Role::ALL.into_iter().find(|r| r.code() == code)
    }

    /// Название роли для человека. Ошибок нет.
    pub fn title(self) -> &'static str {
        match self {
            Role::Control => "Управление",
            Role::General => "Общий",
            Role::Deals => "Сделки",
            Role::Failures => "Неудачные",
        }
    }

    /// Настройка ID чата роли. Ошибок нет.
    pub fn chat_key(self) -> &'static str {
        match self {
            Role::Control => "notify.control_chat",
            Role::General => "notify.telegram_chat",
            Role::Deals => "notify.deals_chat",
            Role::Failures => "notify.failures_chat",
        }
    }

    /// Секрет токена бота роли. Ошибок нет.
    pub fn token_secret(self) -> &'static str {
        match self {
            Role::Control => secrets::TELEGRAM_CONTROL_TOKEN,
            Role::General => secrets::TELEGRAM_BOT_TOKEN,
            Role::Deals => secrets::TELEGRAM_DEALS_TOKEN,
            Role::Failures => secrets::TELEGRAM_FAILURES_TOKEN,
        }
    }

    /// Настройка разрешённых пользователей роли. None — у роли нет кнопок («Сделки»). Ошибок нет.
    pub fn users_key(self) -> Option<&'static str> {
        match self {
            Role::Control => Some("notify.control_users"),
            Role::General => Some("notify.general_users"),
            Role::Deals => None,
            Role::Failures => Some("notify.failures_users"),
        }
    }
}

/// ID чата роли. Принимает: настройки, роль. Возвращает: значение настройки или None — роль не задана. Ошибок нет.
pub fn chat(settings: &Store, role: Role) -> Option<String> {
    settings
        .text(role.chat_key(), At::default())
        .ok()
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
}

/// Роли с заданным ID чата. Принимает: настройки. Ошибок нет.
pub fn configured(settings: &Store) -> Vec<Role> {
    Role::ALL
        .into_iter()
        .filter(|r| chat(settings, *r).is_some())
        .collect()
}

/// Совпадает ли чат с настройкой (числовой id или «@username» — sendMessage принимает оба вида chat_id,
/// https://core.telegram.org/bots/api#sendmessage). Принимает: значение настройки, чат. Ошибок нет.
pub fn chat_matches(setting: &str, chat: &Chat) -> bool {
    if setting == chat.id.to_string() {
        return true;
    }
    match (setting.strip_prefix('@'), chat.username.as_deref()) {
        (Some(want), Some(have)) => want == have,
        _ => false,
    }
}

/// Роль чата входящего обновления среди ролей одного поллера (у каждого токена — свой getUpdates).
/// Принимает: настройки, чат, роли поллера. Возвращает: роль или None — чат не наш. Ошибок нет.
pub fn of_chat(settings: &Store, c: &Chat, among: &[Role]) -> Option<Role> {
    among
        .iter()
        .copied()
        .find(|r| chat(settings, *r).is_some_and(|s| chat_matches(&s, c)))
}

/// Разрешён ли пользователь в роли: список notify.<роль>_users — числовые id или «@username». Список не задан или
/// пуст — любой участник чата роли (как было до решения 176: принимался любой участник notify.telegram_chat).
/// Принимает: настройки, роль, пользователя. Возвращает: true — разрешён. Ошибок нет.
pub fn allowed(settings: &Store, role: Role, user: &User) -> bool {
    let Some(key) = role.users_key() else {
        return false;
    };
    let list = settings.list(key, At::default()).unwrap_or_default();
    if list.iter().all(|x| x.trim().is_empty()) {
        return true;
    }
    let id = user.id.to_string();
    list.iter().map(|x| x.trim()).any(|x| {
        x == id
            || x.strip_prefix('@').is_some_and(|n| {
                user.username
                    .as_deref()
                    .is_some_and(|u| u.eq_ignore_ascii_case(n))
            })
    })
}

/// Куда уходит зеркало «Ручного управления» (решение 156): чат «Неудачные», если он задан, иначе — «Общий» (как раньше).
/// Принимает: настройки. Ошибок нет.
pub fn mirror_role(settings: &Store) -> Role {
    if chat(settings, Role::Failures).is_some() {
        Role::Failures
    } else {
        Role::General
    }
}

/// Один ли это из чатов ролей — для проверки источника команды ядром (core/commands.rs). У ядра есть только числовой
/// id; настройка вида «@username» уже сверена notify (chat_matches) — тогда принимается.
/// Принимает: настройки, числовой id чата строкой. Возвращает: true — чат одной из ролей. Ошибок нет.
pub fn is_role_chat(settings: &Store, chat_id: &str) -> bool {
    Role::ALL
        .into_iter()
        .filter_map(|r| chat(settings, r))
        .any(|s| s == chat_id || s.starts_with('@'))
}
