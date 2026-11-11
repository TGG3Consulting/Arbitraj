//! Файл: notify/ctl/report_sig.rs
//! Что делает: решение 255 (Тигран 11.11) — отчёт «Сигналы и сделки» в «📊 Отчёты» чата «Управление»: экран периодов
//! (за всё время, текущий день, 3 дня, 5 дней, 7 дней, свой интервал — report/sig_period.rs BUTTONS), «Свой интервал» —
//! вопрос с ForceReply (как вопрос суммы перевода, transfer_input.rs), ответ «дд.мм.гг – дд.мм.гг»: разбор строго
//! дд.мм.гг, ошибка — понятный текст и вопрос заново. Отчёт — report::signals_send (сборка потоком в spawn_blocking,
//! торговля не ждёт); файлы — sendDocument по одному по мере сборки (https://core.telegram.org/bots/api#senddocument);
//! больше предела — несколько частей (report/sig_parts.rs, sig_stream.rs), подпись — часть и сигналы с … по …; частей
//! больше одной — итог «файлов N» отдельным сообщением. Сбой — текст в чат и журнал, сборка останавливается.
//! Доступ — как у остальных отчётов (роль Control, notify.control_users).
//! Кто вызывает: notify/ctl/report.rs (r:sig, r:sig.<период>), notify/ctl/press.rs (ответ на вопрос интервала).
//! От чего зависит: report/mod.rs (signals_send, XLSX_MIME), report/sig_period.rs, ctl/kb.rs, ctl/sessions.rs,
//! tg_methods.rs (send_document, force_reply, DOCUMENT_MAX_BYTES).
//! Настройки: нет.

use chrono::Utc;

use crate::report;
use crate::report::sig_period::{BUTTONS, Period, parse_range};
use crate::storage::journal::Level;

use super::super::press::Ctx;
use super::super::tg_buttons::{CallbackQuery, CbMessage, InMessage};
use super::super::tg_methods::{DOCUMENT_MAX_BYTES, force_reply};
use super::kb::{Screen, btn};
use super::sessions::lock;

/// Вопрос своего интервала.
const ASK: &str = "Свой интервал отчёта «Сигналы и сделки»: ответьте на это сообщение двумя датами дд.мм.гг через тире, \
                   например 01.11.26 – 07.11.26 (обе даты включительно, дни по UTC).";

/// Экран периодов «Сигналы и сделки» (по три кнопки в ряд). Принимает: название отчёта. Ошибок нет.
pub fn periods(title: &str) -> Screen {
    let b: Vec<_> = BUTTONS
        .iter()
        .map(|(c, t, _)| btn(t, &format!("r:sig.{c}")))
        .collect();
    let mut s = Screen::new(format!("Отчёт «{title}»: за какой период? (дни — UTC)"));
    for row in b.chunks(3) {
        s = s.row(row.to_vec());
    }
    s.nav(Some("r:m"))
}

impl Ctx<'_> {
    /// «r:sig.<период>»: период — отчёт; «c» — вопрос своего интервала. Ошибок нет: сбой — в чат и журнал.
    pub async fn sig_press(&self, q: &CallbackQuery, msg: &CbMessage, per: &str) {
        let Some((_, _, p)) = BUTTONS.iter().find(|b| b.0 == per) else {
            return self.answer(&q.id, Some("Неизвестная кнопка")).await;
        };
        let who = q.from.describe();
        match p {
            Some(p) => {
                self.answer(&q.id, Some("Готовлю отчёт…")).await;
                self.sig_send(msg.chat.id, *p, &who).await
            }
            None => {
                self.answer(&q.id, Some("Введите интервал ответом на сообщение бота"))
                    .await;
                self.sig_ask(msg.chat.id, None).await
            }
        }
    }

    /// Ответ на вопрос интервала: разбор и отчёт; ошибка — причина и вопрос заново. Ошибок нет.
    pub async fn sig_typed(&self, text: &str, m: &InMessage, who: &str) {
        self.log(
            Level::Info,
            format!("Telegram: {who} ввёл интервал отчёта «{}»", text.trim()),
        )
        .await;
        match parse_range(text) {
            Ok(p) => {
                self.say("Готовлю отчёт…", Some(m.message_id)).await;
                self.sig_send(m.chat.id, p, who).await
            }
            Err(e) => self.sig_ask(m.chat.id, Some(&e)).await,
        }
    }

    /// Вопрос своего интервала с ForceReply (запоминается для ответа). Принимает: чат, причина прошлой ошибки.
    /// Ошибок нет: сбой — в журнал.
    async fn sig_ask(&self, chat: i64, err: Option<&str>) {
        let text = match err {
            Some(e) => format!("Не понял интервал: {e}.\n\n{ASK}"),
            None => ASK.to_string(),
        };
        let r = self
            .tg
            .send_markup(
                &chat.to_string(),
                &text,
                Some(force_reply("дд.мм.гг – дд.мм.гг")),
                None,
            )
            .await;
        match r {
            Ok(id) => {
                lock(self.ctl).range_prompts.insert(chat, id);
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

    /// Собирает отчёт за период и шлёт файлы в чат. Принимает: чат, период, кто запросил. Ошибок нет: сбой — в чат и
    /// журнал.
    async fn sig_send(&self, chat: i64, p: Period, who: &str) {
        let now = Utc::now();
        let (from, to) = p.bounds(now);
        let c = chat.to_string();
        let tg = self.tg;
        let send = |f: report::sig_parts::File| {
            let c = c.clone();
            async move {
                let size = f.bytes.len();
                tg.send_document(&c, &f.name, report::XLSX_MIME, f.bytes, &f.caption)
                    .await
                    .map(|_| ())
                    .map_err(|e| format!("файл {} ({size} байт): {e}", f.name))
            }
        };
        let res = report::signals_send(
            self.core,
            from,
            to,
            p.label(),
            now,
            DOCUMENT_MAX_BYTES,
            send,
        )
        .await;
        match res {
            Ok(sent) => {
                let text = format!(
                    "Отчёт «Сигналы и сделки» ({}) отправлен: файлов {sent}",
                    p.label()
                );
                // Частей больше одной — итог отдельным сообщением (сколько всего, заранее неизвестно).
                if sent > 1
                    && let Err(e) = self.tg.send_markup(&c, &text, None, None).await
                {
                    self.log(
                        Level::Warn,
                        format!("Управление: итог отчёта не отправлен: {e}"),
                    )
                    .await;
                }
                self.log(Level::Info, format!("Управление: {text} по запросу {who}"))
                    .await
            }
            Err(e) => {
                let text = format!("Отчёт «Сигналы и сделки» ({}) не отправлен: {e}", p.label());
                self.log(Level::Error, format!("Управление: {text} (запрос {who})"))
                    .await;
                if let Err(e2) = self.tg.send_markup(&c, &text, None, None).await {
                    self.log(
                        Level::Error,
                        format!("Управление: причина не отправлена: {e2}"),
                    )
                    .await;
                }
            }
        }
    }
}
