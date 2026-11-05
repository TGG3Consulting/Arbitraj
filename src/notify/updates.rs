//! Файл: notify/updates.rs
//! Что делает: приём нажатий кнопок и сообщений Telegram длинным опросом getUpdates (у системы нет публичного
//! HTTPS-адреса для webhook). Решение 176: чатов-ролей несколько (notify/roles.rs), у каждой роли свой токен — опрос
//! ведётся ОДИН на каждый разный токен (роли с одним ботом делят опрос: два getUpdates на один токен Telegram не
//! допускает — второй получает 409 Conflict), входящее обновление по чату относится к своей роли (updates_poll.rs).
//! «Сделки» кнопок нет — эта роль не опрашивается.
//! Набор опросов пересобирается при изменении настроек notify.* / http.timeout_ms и по сигналу wake (разблокировка или
//! смена секретов: сменился токен — у другого бота свои update_id, offset этого токена забывается). offset — по токену,
//! переживает пересборку: подтверждённые обновления не приходят второй раз.
//! Кто вызывает: main.rs (запускает задачу run).
//! От чего зависит: notify/updates_poll.rs (один опрос), notify/roles.rs, notify/client.rs (token_mark),
//! notify/mirror_state.rs, notify/ctl/sessions.rs, core, settings, secrets, storage/journal.
//! Настройки: notify.updates_timeout_s, notify.updates_retry_ms, ID чатов ролей, notify.telegram_api_url,
//! http.timeout_ms; секреты токенов ролей.
//!
//! getUpdates: https://core.telegram.org/bots/api#getupdates

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

use crate::core::Core;
use crate::secrets::Vault;
use crate::settings::live::Watch;
use crate::settings::store::Store;
use crate::storage::journal::{Journal, Level};

use super::client::token_mark;
use super::ctl::sessions::Shared as CtlShared;
use super::mirror_state::Shared;
use super::roles::{self, Role};

/// Префиксы настроек, при изменении которых приём пересобирается.
pub const WATCH_PREFIXES: &[&str] = &["notify.", "http.timeout_ms"];

/// Приём нажатий кнопок Telegram.
pub struct Updates {
    pub http: reqwest::Client,
    pub settings: Store,
    pub vault: Vault,
    pub journal: Journal,
    /// Ядро — команды человека (Core::command) и данные экранов.
    pub core: Core,
    /// Память дубля «Ручного управления» (общая с notify/mirror.rs).
    pub state: Shared,
    /// Память экранов чата «Управление» (общая с notify/ctl/live.rs).
    pub ctl: CtlShared,
    /// Сигнал «секреты изменились» — подаёт api/secrets.rs.
    pub wake: Arc<Notify>,
}

/// offset по отпечатку токена (общий у опросов, переживает пересборку).
pub type Offsets = Mutex<HashMap<u64, i64>>;

/// Роли с кнопками, сгруппированные по токену бота: отпечаток → роли. Роль без чата или без токена не входит.
/// Принимает: настройки, секреты. Возвращает: группы (порядок — по отпечатку) и роли без токена. Ошибок нет.
pub fn groups(settings: &Store, vault: &Vault) -> (BTreeMap<u64, Vec<Role>>, Vec<Role>) {
    let mut g: BTreeMap<u64, Vec<Role>> = BTreeMap::new();
    let mut no_token = Vec::new();
    for r in roles::configured(settings) {
        if r.users_key().is_none() {
            continue;
        }
        match token_mark(vault, r) {
            Some(m) => g.entry(m).or_default().push(r),
            None => no_token.push(r),
        }
    }
    (g, no_token)
}

/// Работает, пока система запущена. Принимает: приём и подписку на настройки (префиксы WATCH_PREFIXES).
/// Ошибок не возвращает: всё пишется в журнал.
pub async fn run(u: Updates, mut watch: Watch) {
    let offsets: Offsets = Mutex::default();
    loop {
        let (g, no_token) = groups(&u.settings, &u.vault);
        for r in &no_token {
            u.log(Level::Warn, format!("Кнопки чата «{}» не принимаются: нет токена бота (секрет {}) или секреты закрыты", r.title(), r.token_secret())).await;
        }
        if g.is_empty() {
            u.log(
                Level::Warn,
                "Кнопки Telegram не принимаются: ни у одной роли чата нет ID чата и токена".into(),
            )
            .await;
        }
        // Опрос, остановленный ошибкой без паузы (notify.updates_retry_ms не задана), ждёт пересборки — как раньше
        // приём ждал изменения настроек; поэтому общий опрос сам не заканчивается.
        let polls = async {
            futures_util::future::join_all(
                g.iter()
                    .map(|(m, rs)| super::updates_poll::run(&u, *m, rs, &offsets)),
            )
            .await;
            std::future::pending::<()>().await
        };
        tokio::select! {
            _ = polls => {}
            _ = u.wake.notified() => {
                // Сменились секреты: offset старых токенов больше не нужен (у нового бота свои update_id).
                offsets.lock().unwrap_or_else(|p| p.into_inner()).clear();
            }
            c = watch.next() => if c.is_none() { return },
        }
    }
}

impl Updates {
    /// Запись в журнал от имени уведомлений. Ошибка журнала уже выведена в лог процесса.
    pub(super) async fn log(&self, level: Level, message: String) {
        let _ = self
            .journal
            .write(level, "notify", None, message, None)
            .await;
    }
}
