//! Файл: notify/dispatch_queue.rs
//! Что делает: решение 340 (Тигран 16.11) — постановка уведомлений Notifier в очередь (notify/queue.rs): снимок карточки
//! сделки на момент события и задание отправки (deliver). Вынесено из dispatch.rs (там — фильтры и сама отправка).
//! Кто вызывает: notify/dispatch.rs (send, send_with_buttons — queued), core/deal_closed.rs (later).
//! От чего зависит: notify/queue.rs (snap, push), notify/dispatch.rs (Notifier, deliver), notify/alert_card.rs (Snap).
//! Настройки: notify.queue_max (читает queue.rs).

use super::dispatch::Notifier;

impl Notifier {
    /// Решение 340: событие в очередь уведомлений — снимок карточки сейчас, отправка (deliver) — фоновой задачей.
    /// Принимает: код, площадку, текст, id объекта кнопок. Ошибок нет.
    pub(super) fn queued(&self, code: &str, venue: Option<&str>, text: &str, object: Option<&str>) {
        let snap = super::queue::snap(code, text);
        let (n, c, v, t, o) = (
            self.clone(),
            code.to_string(),
            venue.map(str::to_string),
            text.to_string(),
            object.map(str::to_string),
        );
        let job = async move {
            n.deliver(&c, v.as_deref(), &t, o.as_deref()).await;
        };
        super::queue::push(self, code, venue, snap, Box::pin(job));
    }

    /// Решение 340: своё задание в очередь уведомлений (по порядку с остальными) — когда текст нужно дособрать фоном
    /// (core/deal_closed.rs: строки «без потолка» — чтение SQLite). Принимает: код и площадку (для записи о потере),
    /// снимок карточки на момент события, задание (внутри — tagged_now / deliver). Ошибок нет.
    pub fn later(
        &self,
        code: &str,
        venue: Option<&str>,
        snap: super::alert_card::Snap,
        job: impl std::future::Future<Output = ()> + Send + 'static,
    ) {
        super::queue::push(self, code, venue, snap, Box::pin(job));
    }
}
