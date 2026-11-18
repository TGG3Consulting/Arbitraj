//! Файл: notify/press_event_ttl.rs
//! Что делает: ответ Тиграна 18.11 (решение 408) — срок кнопок тревог, которые двигают деньги, продают, разворачивают
//! исполнитель или снимают очередь кошелька: не старше notify.dialog_ttl_s по времени сообщения тревоги (поле date
//! сообщения Telegram, https://core.telegram.org/bots/api#message); старше — «кнопка устарела», срок не задан — такие
//! кнопки не работают (как денежные кнопки чата «Управление», решение 176). Вынесено из press_event.rs — размер файла.
//! Кто вызывает: notify/press_event.rs (event_button).
//! От чего зависит: press.rs (Ctx), ctl/press.rs (ttl), tg_buttons.rs (CbMessage).
//! Настройки: notify.dialog_ttl_s (через Ctx::ttl).

use super::press::Ctx;
use super::tg_buttons::CbMessage;

/// Кнопки тревог со сроком notify.dialog_ttl_s: очередь кошелька, продажа, перевод USDT (все шаги), развёртывание.
pub const MONEY: &[&str] = &["qh", "sb", "tu", "tt", "tf", "xd"];

/// Ответ на кнопку тревоги старше срока.
pub const OLD: &str = "Кнопка устарела — нажмите в свежей тревоге или в интерфейсе";

/// Не старше ли сообщение тревоги срока. Принимает: время сообщения (Unix, с), срок, сейчас (Unix, с). Ошибок нет.
pub fn within(sent: i64, ttl: std::time::Duration, now: i64) -> bool {
    now.saturating_sub(sent) <= i64::try_from(ttl.as_secs()).unwrap_or(i64::MAX)
}

impl Ctx<'_> {
    /// Кнопка денежной тревоги не устарела: сообщение не старше notify.dialog_ttl_s. Ошибки: текст для нажавшего
    /// (срок не задан — «кнопки с деньгами не работают»; старше — OLD).
    pub(super) fn fresh(&self, msg: &CbMessage) -> Result<(), String> {
        let ttl = self.ttl()?;
        if within(msg.date, ttl, chrono::Utc::now().timestamp()) {
            Ok(())
        } else {
            Err(OLD.into())
        }
    }
}
