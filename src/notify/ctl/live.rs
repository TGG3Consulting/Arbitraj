//! Файл: notify/ctl/live.rs
//! Что делает: лайв-обновление экранов чата «Управление» (решение 176) — «Состояние» и балансы биржи или кошелька правятся
//! на месте (editMessageText того же сообщения, не новые сообщения), не чаще notify.live_refresh_s и только пока на
//! экране были действия в последние notify.live_idle_s; после — один раз «экран заморожен, нажмите «Обновить»». Живой —
//! только последний экран чата (sessions.rs put), поэтому правок не больше одной на интервал на чат (лимиты Telegram,
//! FAQ Bot API «My bot is hitting limits»: не больше одного сообщения в секунду в чат, 20 в минуту в группу). Правка —
//! только если текст изменился. Данные — из памяти ядра и панели «Балансы» (запрос к бирже — только если ответ старше
//! ui.balances_refresh_ms; не задана — только память). 429 — пауза ровно на parameters.retry_after.
//! Не задана любая из двух настроек — автообновления нет, кнопка «Обновить».
//! Кто вызывает: main.rs (задача run).
//! От чего зависит: ctl/view.rs, ctl/sessions.rs, notify/client.rs, notify/roles.rs, tg_methods.rs, core, settings.
//! Настройки: notify.live_refresh_s, notify.live_idle_s, notify.dialog_ttl_s (уборка), ui.balances_refresh_ms.

use std::time::{Duration, Instant};

use crate::core::Core;
use crate::secrets::Vault;
use crate::settings::live::Watch;
use crate::settings::read::At;
use crate::settings::store::Store;

use super::super::mirror_state::Shared;
use super::super::roles::Role;
use super::super::{client, tg_methods};
use super::sessions::{Sess, Shared as CtlShared, lock};
use super::view;

/// Строка замороженного экрана.
pub const FROZEN: &str = "⏸ Экран заморожен (давно без действий) — нажмите «Обновить»";

/// Что сделать с экраном на тике.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    Refresh,
    Freeze,
}

/// Решение тика по экранам (чистая функция): только живые (View::live) и не замороженные; правленные меньше
/// интервала назад — пропуск (не чаще notify.live_refresh_s); без действий дольше простоя — заморозить.
/// Принимает: экраны (номер, экран), интервал, простой. Возвращает: (номер, действие). Ошибок нет.
pub fn plan(list: &[(u64, Sess)], every: Duration, idle: Duration) -> Vec<(u64, Act)> {
    list.iter()
        .filter(|(_, s)| s.view.live() && !s.frozen)
        .filter(|(_, s)| s.edited.is_none_or(|e| e.elapsed() >= every))
        .map(|(id, s)| {
            (
                *id,
                if s.touched.elapsed() > idle {
                    Act::Freeze
                } else {
                    Act::Refresh
                },
            )
        })
        .collect()
}

/// Лайв-обновление.
pub struct Live {
    pub http: reqwest::Client,
    pub settings: Store,
    pub vault: Vault,
    pub core: Core,
    pub mirror: Shared,
    pub ctl: CtlShared,
}

/// Работает, пока система запущена. Принимает: лайв и подписку на настройки notify.*. Ошибок нет.
pub async fn run(l: Live, mut watch: Watch) {
    loop {
        let get = |n: &str| {
            l.settings
                .integer(n, At::default())
                .ok()
                .map(Duration::from_secs)
        };
        let (every, idle) = (get("notify.live_refresh_s"), get("notify.live_idle_s"));
        let Some(every) = every.filter(|_| idle.is_some()) else {
            if watch.next().await.is_none() {
                return;
            }
            continue;
        };
        tokio::select! {
            _ = tokio::time::sleep(every) => l.tick(every, idle.unwrap_or(every)).await,
            c = watch.next() => if c.is_none() { return },
        }
    }
}

impl Live {
    /// Один тик: уборка устаревших экранов, правка живых, заморозка простаивающих. Ошибок нет.
    async fn tick(&self, every: Duration, idle: Duration) {
        let ttl = self
            .settings
            .integer("notify.dialog_ttl_s", At::default())
            .ok()
            .map(Duration::from_secs);
        let list: Vec<(u64, Sess)> = {
            let mut st = lock(&self.ctl);
            st.purge(ttl.unwrap_or(idle));
            if st.pause_until.is_some_and(|p| Instant::now() < p) {
                return;
            }
            st.sess.iter().map(|(k, v)| (*k, v.clone())).collect()
        };
        let todo = plan(&list, every, idle);
        if todo.is_empty() {
            return;
        }
        let Ok(ready) = client::build_role(&self.http, &self.settings, &self.vault, Role::Control)
        else {
            return;
        };
        for (id, act) in todo {
            let Some((_, s)) = list.iter().find(|(k, _)| *k == id) else {
                continue;
            };
            let sid = lock(&self.ctl).reserve();
            // Лайв не спрашивает биржу чаще интервала панели; не задан — только память.
            let age = Some(self.core.balances_age().unwrap_or(Duration::MAX));
            let (mut screen, refs) =
                view::render(&self.core, &self.settings, &self.mirror, &s.view, sid, age).await;
            if act == Act::Freeze {
                screen.text.push_str(&format!("\n\n{FROZEN}"));
            } else if screen.text == s.shown {
                continue;
            }
            match ready
                .tg
                .edit_text_markup(
                    &s.chat.to_string(),
                    s.msg,
                    &screen.text,
                    Some(screen.markup()),
                )
                .await
            {
                Ok(()) => {
                    let n = Sess {
                        refs,
                        shown: screen.text,
                        frozen: act == Act::Freeze,
                        edited: Some(Instant::now()),
                        ..s.clone()
                    };
                    lock(&self.ctl).put(sid, n);
                }
                Err(e) => {
                    if let Some(sec) = tg_methods::retry_after(&e) {
                        lock(&self.ctl).pause_until =
                            Some(Instant::now() + Duration::from_secs(sec));
                        return;
                    }
                    // Сообщение удалено или не правится — экран больше не живой.
                    if let Some(x) = lock(&self.ctl).sess.get_mut(&id) {
                        x.frozen = true;
                    }
                }
            }
        }
    }
}
