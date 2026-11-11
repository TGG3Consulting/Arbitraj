//! Файл: notify/ctl/report.rs
//! Что делает: решение 215 — кнопка «📊 Отчёты» главного меню чата «Управление»: выбор отчёта («Итоги сделок» или
//! «Упущено»), затем периода («Сутки», «7 дней», «30 дней», «За всё время») — экраны правятся на месте; по периоду —
//! answerCallbackQuery «Готовлю отчёт…», книга Excel той же функцией, что у кнопок интерфейса (report::results_file /
//! missed_file), и файл в тот же чат (sendDocument, https://core.telegram.org/bots/api#senddocument). Больше 50 МБ
//! (предел ботов) или сбой — понятный текст в чат и запись в журнал. Денег не двигает — подтверждения нет.
//! Доступ — как у остальных кнопок чата «Управление»: только роль Control (notify/press.rs accepts) и её разрешённые
//! пользователи (notify.control_users).
//! Периоды — кнопки интерфейса, а не пороги (как «сутки / 7 дней» экрана «Упущено из-за потолка»,
//! ui/src/lib/uncappedLabels.js period: последние 24 ч / 7 × 24 ч до сейчас; «30 дней» и «за всё время» — решение 215).
//! Кто вызывает: notify/press.rs («r:<что>»).
//! От чего зависит: report/mod.rs, ctl/kb.rs, ctl/report_sig.rs, ctl/press.rs (put_screen), tg_methods.rs (send_document), core.
//! Настройки: нет.
//!
//! callback_data (≤ 64 байт): «r:m» — выбор отчёта; «r:<отчёт>» — выбор периода; «r:<отчёт>.<период>» — файл.
//! Отчёт: res — «Итоги сделок», mis — «Упущено». Период: d, w, m, a (см. PERIODS).
//! Решение 255: sig — «Сигналы и сделки», свои периоды (report/sig_period.rs BUTTONS: a, t, 3, 5, 7, c — свой
//! интервал вводом «дд.мм.гг – дд.мм.гг») и отправка частями — notify/ctl/report_sig.rs.

use chrono::{Duration, Utc};

use crate::report;
use crate::storage::journal::Level;

use super::super::press::Ctx;
use super::super::tg_buttons::{CallbackQuery, CbMessage};
use super::kb::{Screen, btn};

/// Отчёты: код, название, префикс файла.
const REPORTS: [(&str, &str, &str); 3] = [
    ("res", "Итоги сделок", "arb_results"),
    ("mis", "Упущено", "arb_missed"),
    // Решение 255: свои периоды и ввод своего интервала — report_sig.rs.
    ("sig", "Сигналы и сделки", "arb_signals"),
];

/// Периоды: код, надпись, сколько суток назад от сейчас (None — за всё время). Кнопки интерфейса (см. шапку).
const PERIODS: [(&str, &str, Option<i64>); 4] = [
    ("d", "Сутки", Some(1)),
    ("w", "7 дней", Some(7)),
    ("m", "30 дней", Some(30)),
    ("a", "За всё время", None),
];

/// Экран «Отчёты»: какой отчёт. Ошибок нет.
pub fn choose() -> Screen {
    let mut s = Screen::new("📊 Отчёты Excel: какой отчёт прислать?");
    for (code, title, _) in REPORTS {
        s = s.one(btn(title, &format!("r:{code}")));
    }
    s.nav(None)
}

/// Экран периода отчёта. Принимает: код отчёта. Возвращает: экран или None — нет такого отчёта. Ошибок нет.
pub fn periods(code: &str) -> Option<Screen> {
    let (_, title, _) = REPORTS.iter().find(|r| r.0 == code)?;
    if code == "sig" {
        return Some(super::report_sig::periods(title));
    }
    let row = PERIODS
        .iter()
        .map(|(p, t, _)| btn(t, &format!("r:{code}.{p}")))
        .collect();
    Some(
        Screen::new(format!("Отчёт «{title}»: за какой период?"))
            .row(row)
            .nav(Some("r:m")),
    )
}

impl Ctx<'_> {
    /// «r:<что>» — экраны отчётов и отправка файла. Ошибок нет: сбой — в чат и журнал.
    pub async fn ctl_report(&self, q: &CallbackQuery, msg: &CbMessage, object: &str) {
        let screen = match object.split_once('.') {
            None if object == "m" => Some(choose()),
            None => periods(object),
            Some(("sig", per)) => return self.sig_press(q, msg, per).await,
            Some((rep, per)) => return self.report_send(q, msg, rep, per).await,
        };
        let Some(s) = screen else {
            return self.answer(&q.id, Some("Неизвестная кнопка")).await;
        };
        self.answer(&q.id, None).await;
        self.put_screen(msg.chat.id, Some(msg.message_id), &s).await;
    }

    /// Собирает отчёт за период и шлёт файлом в чат нажатия. Принимает: нажатие, сообщение, коды отчёта и периода.
    /// Ошибок нет: сбой — текст в чат и журнал.
    async fn report_send(&self, q: &CallbackQuery, msg: &CbMessage, rep: &str, per: &str) {
        let (Some(r), Some(p)) = (
            REPORTS.iter().find(|x| x.0 == rep),
            PERIODS.iter().find(|x| x.0 == per),
        ) else {
            return self.answer(&q.id, Some("Неизвестная кнопка")).await;
        };
        self.answer(&q.id, Some("Готовлю отчёт…")).await;
        let now = Utc::now();
        let from = p.2.map(|days| now - Duration::days(days));
        let built = match rep {
            "res" => report::results_file(self.core, from, None, now).await,
            _ => report::missed_file(self.core, from, None, now).await,
        };
        let who = q.from.describe();
        let chat = msg.chat.id.to_string();
        let name = report::file_name(r.2, now);
        let caption = format!("Отчёт «{}» — {}", r.1, report::period_text(from, None));
        let sent = match built {
            Ok(bytes) => self
                .tg
                .send_document(&chat, &name, report::XLSX_MIME, bytes, &caption)
                .await
                .map_err(|e| e.to_string()),
            Err(e) => Err(e),
        };
        match sent {
            Ok(_) => {
                self.log(
                    Level::Info,
                    format!(
                        "Управление: отчёт «{}» ({}) отправлен по запросу {who}",
                        r.1, p.1
                    ),
                )
                .await
            }
            Err(e) => {
                let text = format!("Отчёт «{}» ({}) не отправлен: {e}", r.1, p.1);
                self.log(Level::Error, format!("Управление: {text} (запрос {who})"))
                    .await;
                if let Err(e2) = self.tg.send_markup(&chat, &text, None, None).await {
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
