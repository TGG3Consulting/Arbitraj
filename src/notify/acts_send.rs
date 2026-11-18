//! Файл: notify/acts_send.rs
//! Что делает: решение 408 (t193) — отправка события с кнопками тревоги (notify/acts.rs; вынесено — размер файла):
//! Notifier::send_acts (как send) и send_tagged_acts (как send_tagged — и в чаты ролей). Не ждут Telegram — задание
//! очереди уведомлений (notify/queue.rs) с кнопками в task-local (acts::scoped); deliver и to_role берут их оттуда.
//! Кто вызывает: core/deps.rs (notify_acts, notify_tagged_acts).
//! От чего зависит: notify/acts.rs, queue.rs, dispatch.rs (deliver), dispatch_role.rs (tagged_now), glue.rs.
//! Настройки: как у send / send_tagged (notify.event.*, notify.queue_max).

use super::acts::{Act, scoped};
use super::dispatch::Notifier;
use super::routes::Tag;

impl Notifier {
    /// Как send, с кнопками события (пусто — обычный send). Не ждёт Telegram — задание в очередь. Ошибок нет.
    pub async fn send_acts(&self, code: &str, venue: Option<&str>, text: &str, acts: Vec<Act>) {
        if acts.is_empty() || super::glue::glued(code) {
            return self.send(code, venue, text).await;
        }
        let snap = super::queue::snap(code, text);
        let (n, c, v, t) = (
            self.clone(),
            code.to_string(),
            venue.map(str::to_string),
            text.to_string(),
        );
        let job = async move {
            scoped(acts, n.deliver(&c, v.as_deref(), &t, None)).await;
        };
        super::queue::push(self, code, venue, snap, Box::pin(job));
    }

    /// Как send_tagged (и в чаты ролей), с кнопками события. Ошибок нет.
    pub async fn send_tagged_acts(
        &self,
        code: &str,
        venue: Option<&str>,
        text: &str,
        tag: Tag,
        acts: Vec<Act>,
    ) {
        let snap = super::queue::snap(code, text);
        let (n, c, v, t) = (
            self.clone(),
            code.to_string(),
            venue.map(str::to_string),
            text.to_string(),
        );
        let job = async move {
            scoped(acts, n.tagged_now(&c, v.as_deref(), &t, tag)).await;
        };
        super::queue::push(self, code, venue, snap, Box::pin(job));
    }
}
