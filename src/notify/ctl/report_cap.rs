//! Файл: notify/ctl/report_cap.rs
//! Что делает: решение 256 (Тигран 11.11) — отчёт «Сделки, лимиты и капитал» в «📊 Отчёты» чата «Управление»: экран
//! периодов (за всё время, текущий день, 3 дня, 5 дней, 7 дней, свой интервал — те же кнопки, что у «Сигналов и
//! сделок»: report/sig_period.rs BUTTONS; окно — календарными днями UTC, как «Итоги сделок»: Period::calendar), «Свой
//! интервал» — вопрос с ForceReply (как вопрос суммы перевода, transfer_input.rs), ответ «дд.мм.гг – дд.мм.гг»: разбор
//! строго дд.мм.гг, ошибка — понятный текст и вопрос заново. Отчёт — report::cap_send::capital_send (сборка в
//! spawn_blocking, торговля не ждёт); файлы — sendDocument по одному по мере сборки
//! (https://core.telegram.org/bots/api#senddocument); больше предела DOCUMENT_MAX_BYTES — несколько частей по дням
//! (report/cap_xlsx.rs files, каждая — сразу по сборке), подпись — окно, часть и дни; частей больше одной — итог
//! «файлов N» отдельным сообщением. Сбой —
//! текст в чат и журнал. Денег не двигает — подтверждения нет. Доступ — как у остальных отчётов (роль Control,
//! notify.control_users).
//! Кто вызывает: notify/ctl/report.rs (r:cap, r:cap.<период>), notify/ctl/press.rs (ответ на вопрос интервала).
//! От чего зависит: report/cap_send.rs, report/sig_period.rs, report/mod.rs (XLSX_MIME), ctl/kb.rs, ctl/sessions.rs,
//! tg_methods.rs (send_document, force_reply, DOCUMENT_MAX_BYTES).
//! Настройки: нет (потолки модели — uncapped.ladder_usdt, читает ядро).

use chrono::Utc;

use crate::report;
use crate::report::sig_period::{BUTTONS, Period, parse_range};
use crate::storage::journal::Level;

use super::super::press::Ctx;
use super::super::tg_buttons::{CallbackQuery, CbMessage, InMessage};
use super::super::tg_methods::{DOCUMENT_MAX_BYTES, force_reply};
use super::kb::{Screen, btn};
use super::sessions::lock;

/// Название отчёта в сообщениях.
const NAME: &str = "Сделки, лимиты и капитал";

/// Вопрос своего интервала.
const ASK: &str = "Свой интервал отчёта «Сделки, лимиты и капитал»: ответьте на это сообщение двумя датами дд.мм.гг \
                   через тире, например 01.11.26 – 07.11.26 (обе даты включительно, дни по UTC).";

/// Экран периодов (по три кнопки в ряд). Принимает: название отчёта. Ошибок нет.
pub fn periods(title: &str) -> Screen {
    let b: Vec<_> = BUTTONS
        .iter()
        .map(|(c, t, _)| btn(t, &format!("r:cap.{c}")))
        .collect();
    let mut s = Screen::new(format!("Отчёт «{title}»: за какой период? (дни — UTC)"));
    for row in b.chunks(3) {
        s = s.row(row.to_vec());
    }
    s.nav(Some("r:m"))
}

impl Ctx<'_> {
    /// «r:cap.<период>»: период — отчёт; «c» — вопрос своего интервала. Ошибок нет: сбой — в чат и журнал.
    pub async fn cap_press(&self, q: &CallbackQuery, msg: &CbMessage, per: &str) {
        let Some((_, _, p)) = BUTTONS.iter().find(|b| b.0 == per) else {
            return self.answer(&q.id, Some("Неизвестная кнопка")).await;
        };
        let who = q.from.describe();
        match p {
            Some(p) => {
                self.answer(&q.id, Some("Готовлю отчёт…")).await;
                self.cap_send(msg.chat.id, *p, &who).await
            }
            None => {
                self.answer(&q.id, Some("Введите интервал ответом на сообщение бота"))
                    .await;
                self.cap_ask(msg.chat.id, None).await
            }
        }
    }

    /// Это ответ на наш вопрос своего интервала? Да — вопрос забывается. Принимает: сообщение. Ошибок нет.
    pub fn cap_reply(&self, m: &InMessage) -> bool {
        let Some(r) = m.reply_to_message.as_ref() else {
            return false;
        };
        let mut st = lock(self.ctl);
        let hit = st.cap_prompts.get(&m.chat.id) == Some(&r.message_id);
        if hit {
            st.cap_prompts.remove(&m.chat.id);
        }
        hit
    }

    /// Ответ на вопрос интервала: разбор и отчёт; ошибка — причина и вопрос заново. Ошибок нет.
    pub async fn cap_typed(&self, text: &str, m: &InMessage, who: &str) {
        self.log(
            Level::Info,
            format!(
                "Telegram: {who} ввёл интервал отчёта «{NAME}»: «{}»",
                text.trim()
            ),
        )
        .await;
        match parse_range(text) {
            Ok(p) => {
                self.say("Готовлю отчёт…", Some(m.message_id)).await;
                self.cap_send(m.chat.id, p, who).await
            }
            Err(e) => self.cap_ask(m.chat.id, Some(&e)).await,
        }
    }

    /// Вопрос своего интервала с ForceReply (запоминается для ответа). Принимает: чат, причина прошлой ошибки.
    /// Ошибок нет: сбой — в журнал.
    async fn cap_ask(&self, chat: i64, err: Option<&str>) {
        let text = match err {
            Some(e) => format!("Не понял интервал: {e}.\n\n{ASK}"),
            None => ASK.to_string(),
        };
        let markup = Some(force_reply("дд.мм.гг – дд.мм.гг"));
        match self
            .tg
            .send_markup(&chat.to_string(), &text, markup, None)
            .await
        {
            Ok(id) => {
                lock(self.ctl).cap_prompts.insert(chat, id);
            }
            Err(e) => {
                self.log(
                    Level::Error,
                    format!("Отчёты: вопрос интервала не отправлен: {e}"),
                )
                .await
            }
        }
    }

    /// Собирает отчёт за период и шлёт файлы в чат. Принимает: чат, период, кто запросил. Ошибок нет: сбой — в чат
    /// и журнал.
    async fn cap_send(&self, chat: i64, p: Period, who: &str) {
        // Решение 256 (дополнение, аудит п. 7): календарные дни UTC, как «Итоги сделок» (sig_period.rs calendar).
        let w = p.calendar(Utc::now());
        let c = chat.to_string();
        let tg = self.tg;
        let send = |f: report::cap_xlsx::File| {
            let c = c.clone();
            async move {
                let size = f.bytes.len();
                tg.send_document(&c, &f.name, report::XLSX_MIME, f.bytes, &f.caption)
                    .await
                    .map(|_| ())
                    .map_err(|e| format!("файл {} ({size} байт): {e}", f.name))
            }
        };
        let res =
            report::cap_send::capital_send(self.core, w, p.label(), DOCUMENT_MAX_BYTES, send).await;
        // В чат: сбой — всегда; успех — только если частей больше одной (сколько всего, заранее неизвестно).
        let (level, text, tell) = match res {
            Ok(sent) => (
                Level::Info,
                format!("Отчёт «{NAME}» ({}) отправлен: файлов {sent}", p.label()),
                sent > 1,
            ),
            Err(e) => (
                Level::Error,
                format!("Отчёт «{NAME}» ({}) не отправлен: {e}", p.label()),
                true,
            ),
        };
        if tell && let Err(e) = self.tg.send_markup(&c, &text, None, None).await {
            self.log(
                Level::Warn,
                format!("Управление: итог отчёта не отправлен: {e}"),
            )
            .await;
        }
        self.log(level, format!("Управление: {text} (запрос {who})"))
            .await
    }
}
