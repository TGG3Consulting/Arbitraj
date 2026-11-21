//! Файл: notify/check.rs
//! Что делает: кнопка «Проверить подключение» для Telegram: проверяет токен (getMe) и доступ бота к чату (getChat);
//! у ролей чатов (решение 176) — ещё и тестовое сообщение в чат роли (sendMessage): бот должен уметь туда писать.
//! Ответ — «Подключено» или точная причина сбоя.
//! Кто вызывает: api/check.rs (POST /api/check/telegram — «Общий» как раньше, POST /api/check/telegram/{роль}).
//! От чего зависит: notify/client.rs, notify/telegram.rs, notify/roles.rs.
//! Настройки: notify.telegram_api_url, http.timeout_ms, ID чата роли; секрет токена роли.

use crate::gateway::contract::ConnectionCheck;
use crate::secrets::Vault;
use crate::settings::store::Store;

use super::client;
use super::roles::Role;

/// Проверяет подключение «Общего» чата (как до решения 176: getMe и getChat, без тестового сообщения).
/// Принимает: HTTP-клиент, настройки, секреты. Возвращает: ConnectionCheck. Не падает.
pub async fn telegram(http: &reqwest::Client, settings: &Store, vault: &Vault) -> ConnectionCheck {
    role(http, settings, vault, Role::General, false).await
}

/// Проверяет подключение роли: getMe, getChat и (если send) тестовое сообщение в чат роли.
/// Принимает: HTTP-клиент, настройки, секреты, роль, слать ли тест.
/// Возвращает: ConnectionCheck — ok=true «Подключено: бот @имя, чат …», иначе точная причина. Не падает.
pub async fn role(
    http: &reqwest::Client,
    settings: &Store,
    vault: &Vault,
    role: Role,
    send: bool,
) -> ConnectionCheck {
    let fail = |details: String| ConnectionCheck { ok: false, details };
    let ready = match client::build_role(http, settings, vault, role) {
        Ok(r) => r,
        Err(e) => return fail(format!("Чат «{}»: {}", role.title(), e.message)),
    };
    let bot = match ready.tg.get_me().await {
        Ok(name) => name,
        Err(e) => return fail(format!("Чат «{}»: токен бота: {e}", role.title())),
    };
    let chat = match ready.tg.get_chat(&ready.chat).await {
        Ok(c) => c,
        Err(e) => {
            return fail(format!(
                "Бот @{bot} работает, но чат {} («{}») недоступен: {e}",
                ready.chat,
                role.title()
            ));
        }
    };
    if send {
        let text = format!(
            "Проверка подключения ARB: этот чат — «{}». Сообщение можно удалить.",
            role.title()
        );
        if let Err(e) = ready.tg.send(&ready.chat, &text).await {
            return fail(format!(
                "Бот @{bot} видит чат «{chat}», но писать туда не может: {e}"
            ));
        }
    }
    ConnectionCheck {
        ok: true,
        details: format!(
            "Подключено: бот @{bot}, чат «{chat}» — роль «{}»",
            role.title()
        ),
    }
}
