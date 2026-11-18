//! Файл: notify/press_event_ttl.rs
//! Что делает: ответ Тиграна 18.11 (решение 408) — срок кнопок тревог, которые двигают деньги, продают, разворачивают
//! исполнитель или снимают очередь кошелька: не старше notify.dialog_ttl_s по времени сообщения тревоги (поле date
//! сообщения Telegram, https://core.telegram.org/bots/api#message); старше — «кнопка устарела», срок не задан — такие
//! кнопки не работают (как денежные кнопки чата «Управление», решение 176). Вынесено из press_event.rs — размер файла.
//! Кто вызывает: notify/press_event.rs (event_button).
//! От чего зависит: press.rs (Ctx), ctl/press.rs (ttl).
//! Настройки: notify.dialog_ttl_s (через Ctx::ttl).

use super::press::Ctx;

/// Кнопки тревог со сроком notify.dialog_ttl_s: очередь кошелька, продажа, перевод USDT (все шаги), развёртывание.
pub const MONEY: &[&str] = &["qh", "sb", "tu", "tt", "tf", "tx", "xd"];

/// Ревью t193 (М1): время тревоги и данные кнопки. tf, tx — «<время тревоги>.<данные>» (шаги перевода, кнопки ставит
/// press_event_money.rs), остальные — время сообщения с кнопкой. Принимает: что, данные, время сообщения (Unix, с).
/// Возвращает: (время тревоги, данные без него) или None — неверные данные. Ошибок нет.
pub fn alert_time<'a>(op: &str, arg: &'a str, msg: i64) -> Option<(i64, &'a str)> {
    if !matches!(op, "tf" | "tx") {
        return Some((msg, arg));
    }
    let (t, rest) = arg.split_once('.')?;
    Some((t.parse().ok()?, rest))
}

/// Ответ на кнопку тревоги старше срока.
pub const OLD: &str = "Кнопка устарела — нажмите в свежей тревоге или в интерфейсе";

/// Ревью t193 (Н1): «Да» этих команд — тоже не старше срока, по времени сообщения вопроса (press_confirm.rs).
/// Принимает: действие. Возвращает: true — денежное. Ошибок нет.
pub fn money_action(a: &crate::core::types::ManualAction) -> bool {
    use crate::core::types::ManualAction as A;
    matches!(
        a,
        A::SellBalance { .. }
            | A::TransferUsdt { .. }
            | A::ReleaseWalletQueue { .. }
            | A::DeployExecutor { .. }
    )
}

/// Не старше ли сообщение тревоги срока. Принимает: время сообщения (Unix, с), срок, сейчас (Unix, с). Ошибок нет.
pub fn within(sent: i64, ttl: std::time::Duration, now: i64) -> bool {
    now.saturating_sub(sent) <= i64::try_from(ttl.as_secs()).unwrap_or(i64::MAX)
}

impl Ctx<'_> {
    /// Кнопка денежной тревоги не устарела: не старше notify.dialog_ttl_s по времени (Unix, с) — тревоги (и
    /// переданному по цепочке шагов перевода, ревью t193 М1) или вопроса «Да» (ревью t193 Н1). Ошибки: текст для
    /// нажавшего (срок не задан — «кнопки с деньгами не работают»; старше — OLD).
    pub(super) fn fresh_at(&self, sent: i64) -> Result<(), String> {
        let ttl = self.ttl()?;
        if within(sent, ttl, chrono::Utc::now().timestamp()) {
            Ok(())
        } else {
            Err(OLD.into())
        }
    }
}
