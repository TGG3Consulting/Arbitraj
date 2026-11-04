//! Файл: notify/tg_commands.rs
//! Что делает: сообщения в Telegram (решение 157) — только из настроенного чата notify.telegram_chat:
//! команды /help, /menu и /dev (меню управления), /status, /manual, /start, /resume, /pause, /stop (короткие пути
//! плашки режима — с подтверждением «Да, …»); в группе «/cmd@другой_бот» — не нам, игнор. Плюс ответ человека на вопрос
//! суммы «Продано вручную» (ForceReply, решение 156) — дальше подтверждение «Да, …» с этой суммой.
//! Список команд регистрируется setMyCommands (подсказка по «/», https://core.telegram.org/bots/api#setmycommands).
//! Кто вызывает: notify/updates.rs — на каждое Update с message.
//! От чего зависит: press.rs (Ctx, chat_matches), press_confirm.rs, press_record.rs, menu_press.rs, mirror_state.rs.
//! Настройки: нет.

use crate::core::risk::Mode;
use crate::storage::journal::Level;

use super::menu_press::control_action;
use super::mirror_state::{Pending, lock};
use super::press::{Ctx, chat_matches};
use super::press_record::ALREADY;
use super::tg_buttons::InMessage;

/// Команды бота: (команда без «/», описание) — для /help и setMyCommands.
pub const COMMANDS: &[(&str, &str)] = &[
    ("help", "Список команд"),
    (
        "menu",
        "Меню управления: состояние, ручное управление, режим, площадки, лимит дня",
    ),
    ("dev", "То же, что /menu"),
    (
        "status",
        "Состояние системы — как плашка режима в интерфейсе",
    ),
    (
        "manual",
        "Ручное управление: записи, ждущие действия, с их кнопками",
    ),
    ("start", "Старт — система в работу (с подтверждением)"),
    (
        "resume",
        "То же, что /start: снять паузу или стоп (с подтверждением)",
    ),
    (
        "pause",
        "Пауза — новые сделки не открываются, идущие доводятся (с подтверждением)",
    ),
    (
        "stop",
        "Стоп — новые сделки не открываются (с подтверждением)",
    ),
];

/// Разбирает команду. Принимает: текст, имя бота. Возвращает: команду строчными без «/» или None — не команда или
/// адресована другому боту («/cmd@другой»).
pub fn parse(text: &str, bot: &str) -> Option<String> {
    let word = text.split_whitespace().next()?.strip_prefix('/')?;
    let (cmd, to) = word
        .split_once('@')
        .map_or((word, None), |(c, b)| (c, Some(b)));
    if to.is_some_and(|b| !b.eq_ignore_ascii_case(bot)) || cmd.is_empty() {
        return None;
    }
    Some(cmd.to_lowercase())
}

/// Текст /help.
pub fn help() -> String {
    let mut t = String::from("Команды бота ARB (изменение настроек — только в интерфейсе):");
    for (c, d) in COMMANDS {
        t.push_str(&format!("\n/{c} — {d}"));
    }
    t
}

impl Ctx<'_> {
    /// Обрабатывает сообщение. Ошибок не возвращает: всё — в журнал и ответом в чат.
    pub async fn message(&self, m: InMessage) {
        let who = m
            .from
            .as_ref()
            .map(|u| u.describe())
            .unwrap_or_else(|| "без автора".into());
        if !chat_matches(self.chat, &m.chat) {
            return self
                .log(
                    Level::Warn,
                    format!("Сообщение из чужого чата {} от {who} — игнор", m.chat.id),
                )
                .await;
        }
        let text = m.text.clone().unwrap_or_default();
        let prompt = m
            .reply_to_message
            .as_ref()
            .and_then(|r| lock(self.state).prompts.remove(&r.message_id));
        if let Some(p) = prompt {
            return self
                .amount(&p.key, &p.code, &text, m.message_id, &who)
                .await;
        }
        let Some(cmd) = parse(&text, self.bot) else {
            return;
        };
        self.log(Level::Info, format!("Telegram: {who} — команда /{cmd}"))
            .await;
        let mode = match cmd.as_str() {
            "help" => return self.say(&help(), Some(m.message_id)).await,
            "menu" | "dev" => return self.send_screen("root").await,
            "status" => return self.send_screen("st").await,
            "manual" => return self.send_screen("man").await,
            "start" | "resume" => Mode::Running,
            "pause" => Mode::Paused,
            "stop" => Mode::Stopped,
            _ => {
                return self
                    .say(
                        &format!("Неизвестная команда /{cmd}. Список — /help"),
                        Some(m.message_id),
                    )
                    .await;
            }
        };
        match control_action(&self.core.control(), &format!("mode.{mode:?}")) {
            Ok(p) => self.ask("", Some(m.message_id), p).await,
            Err(why) => {
                self.say(&format!("Недоступно: {why}"), Some(m.message_id))
                    .await
            }
        }
    }

    /// Ответ на вопрос суммы: подтверждение «Да, …» с этой суммой (число проверит ядро, как у экрана).
    /// Принимает: ключ записи, код кнопки, текст ответа, id сообщения, кто. Ошибок нет.
    async fn amount(&self, key: &str, code: &str, text: &str, reply_to: i64, who: &str) {
        let Some(b) = self.live_record(key).and_then(|r| r.button(code).cloned()) else {
            return self.say(ALREADY, Some(reply_to)).await;
        };
        self.log(
            Level::Info,
            format!(
                "Telegram: {who} ввёл сумму «{}» для «{}» ({key})",
                text.trim(),
                b.title
            ),
        )
        .await;
        let p = Pending {
            key: Some(key.into()),
            action: b.action_with(Some(text)),
            title: b.title.clone(),
            question: b.confirm_with(Some(text)),
        };
        self.ask("", Some(reply_to), p).await;
    }

    /// Простое сообщение в чат (ответом на сообщение). Ошибка — в журнал.
    async fn say(&self, text: &str, reply_to: Option<i64>) {
        if let Err(e) = self.tg.send_markup(self.chat, text, None, reply_to).await {
            self.log(Level::Error, format!("Ответ в Telegram не отправлен: {e}"))
                .await;
        }
    }
}
