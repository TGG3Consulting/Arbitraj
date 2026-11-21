//! Файл: notify/ctl/press.rs
//! Что делает: нажатия и сообщения чата «Управление» (решение 176): «k:<экран>» — экран правится на месте
//! (editMessageText, https://core.telegram.org/bots/api#editmessagetext), «s:<номер>.<строка>» — «Продать» монету из
//! «Балансов»: доступность — заново из памяти панели (решение 174, та же причина), затем вопрос «Да, продать …» с
//! количеством, монетой, счётом и биржей (press_confirm.rs ask → та же команда ядра SellBalance, что у панели);
//! сообщения: /menu (/start, /help) — главное меню новым сообщением, ответ на вопрос суммы — в transfer_press.rs,
//! ответ на вопрос своего интервала отчёта «Сигналы и сделки» — в report_sig.rs (решение 255).
//! Устаревшая денежная кнопка (после перезапуска, старше notify.dialog_ttl_s или экран уже перерисован) — «экран
//! устарел, откройте меню», деньги не двигаются.
//! «Продать» — sell.rs.
//! Решение 256: ответ на вопрос своего интервала отчёта «Сделки, лимиты и капитал» — в report_cap.rs (cap_typed).
//! Кто вызывает: notify/press.rs, notify/tg_commands.rs (ctl_message).
//! От чего зависит: ctl/view.rs, ctl/sessions.rs, ctl/kb.rs, press_confirm.rs (ask), core (venue_balances), tg_methods.rs.
//! Настройки: notify.dialog_ttl_s, ui.balances_refresh_ms (возраст балансов, через Core::balances_age).

use std::time::{Duration, Instant};

use crate::settings::read::At;
use crate::storage::journal::Level;

use super::super::press::Ctx;
use super::super::tg_buttons::{CallbackQuery, CbMessage, InMessage};
use super::super::tg_commands::parse;
use super::kb::Screen;
use super::sessions::{Sess, View, lock};
use super::view;

/// Ответ на устаревшую денежную кнопку.
pub const STALE: &str = "Экран устарел — откройте меню (/menu) и выберите снова";

impl Ctx<'_> {
    /// Срок экранов и диалогов notify.dialog_ttl_s. Ошибка: не задана (текст для нажавшего).
    pub fn ttl(&self) -> Result<Duration, String> {
        self.settings
            .integer("notify.dialog_ttl_s", At::default())
            .map(Duration::from_secs)
            .map_err(|e| format!("Кнопки с деньгами не работают: {}", e.message))
    }

    /// Экран по номеру «<номер>.<остаток>» из callback_data, если он не устарел (и отметка «трогали»).
    /// Возвращает: (номер, экран, остаток). Ошибки: текст для нажавшего (устарел, срок не задан).
    pub fn ctl_get<'o>(&self, object: &'o str) -> Result<(u64, Sess, &'o str), String> {
        let ttl = self.ttl()?;
        let (id, rest) = object.split_once('.').unwrap_or((object, ""));
        let id: u64 = id.parse().map_err(|_| STALE.to_string())?;
        let mut st = lock(self.ctl);
        let s = st.get(id, ttl).ok_or_else(|| STALE.to_string())?;
        if let Some(x) = st.sess.get_mut(&id) {
            x.touched = Instant::now();
        }
        Ok((id, s, rest))
    }

    /// «k:<экран>» — экран на месте нажатого сообщения. Ошибок нет.
    pub async fn ctl_nav(&self, q: &CallbackQuery, msg: &CbMessage, object: &str) {
        let Some(v) = view::parse(object) else {
            return self.answer(&q.id, Some("Неизвестный экран")).await;
        };
        self.answer(&q.id, None).await;
        if v == View::Transfer {
            return self.transfer_start(msg.chat.id, Some(msg.message_id)).await;
        }
        self.show(msg.chat.id, Some(msg.message_id), v).await;
    }

    /// Показывает экран: правит сообщение (msg) или шлёт новое, запоминает отрисовку. Ошибок нет: сбой — в журнал.
    pub async fn show(&self, chat: i64, msg: Option<i64>, v: View) {
        let sid = lock(self.ctl).reserve();
        let age = self.core.balances_age();
        let (screen, refs) = view::render(self.core, self.settings, self.state, &v, sid, age).await;
        let Some(msg) = self.put_screen(chat, msg, &screen).await else {
            return;
        };
        let s = Sess {
            chat,
            msg,
            view: v,
            refs,
            dialog: None,
            touched: Instant::now(),
            shown: screen.text,
            frozen: false,
            edited: None,
        };
        lock(self.ctl).put(sid, s);
    }

    /// Правит сообщение экрана (msg) или шлёт новое. «message is not modified» — не ошибка (экран тот же).
    /// Возвращает: id сообщения экрана или None — не показан (в журнал). Ошибок нет.
    pub async fn put_screen(&self, chat: i64, msg: Option<i64>, s: &Screen) -> Option<i64> {
        let c = chat.to_string();
        if let Some(id) = msg {
            match self
                .tg
                .edit_text_markup(&c, id, &s.text, Some(s.markup()))
                .await
            {
                Ok(()) => return Some(id),
                Err(e) if e.to_string().contains("not modified") => return Some(id),
                Err(e) => {
                    self.log(
                        Level::Warn,
                        format!("Управление: экран не исправлен, шлю новым: {e}"),
                    )
                    .await
                }
            }
        }
        match self
            .tg
            .send_markup(&c, &s.text, Some(s.markup()), None)
            .await
        {
            Ok(id) => Some(id),
            Err(e) => {
                self.log(Level::Error, format!("Управление: экран не отправлен: {e}"))
                    .await;
                None
            }
        }
    }

    /// Сообщение в чате «Управление»: ответ на вопрос суммы перевода или /menu. Ошибок нет.
    pub async fn ctl_message(&self, m: &InMessage, text: &str, who: &str) {
        // Решение 256: ответ на вопрос своего интервала отчёта «Сделки, лимиты и капитал».
        if self.cap_reply(m) {
            return self.cap_typed(text, m, who).await;
        }
        let prompt = m
            .reply_to_message
            .as_ref()
            .and_then(|r| lock(self.ctl).prompts.remove(&(m.chat.id, r.message_id)));
        if let Some(sid) = prompt {
            return self.transfer_typed(sid, text, m, who).await;
        }
        // Решение 255: ответ на вопрос своего интервала отчёта «Сигналы и сделки».
        let range = m.reply_to_message.as_ref().is_some_and(|r| {
            let mut st = lock(self.ctl);
            let hit = st.range_prompts.get(&m.chat.id) == Some(&r.message_id);
            if hit {
                st.range_prompts.remove(&m.chat.id);
            }
            hit
        });
        if range {
            return self.sig_typed(text, m, who).await;
        }
        match parse(text, self.bot).as_deref() {
            Some("menu" | "start" | "help") => self.show(m.chat.id, None, View::Root).await,
            Some(_) => {
                self.say(
                    "Управление — кнопками: откройте меню командой /menu",
                    Some(m.message_id),
                )
                .await
            }
            None => {}
        }
    }
}
