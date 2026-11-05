//! Файл: notify/menu_press.rs
//! Что делает: нажатия в меню управления Telegram (решение 157): «m:<экран>» — экран меню правится на месте
//! (editMessageText с новыми кнопками, https://core.telegram.org/bots/api#editmessagetext); «c:<что>» — кнопка
//! плашки режима: проверка доступности (как на плашке) и вопрос подтверждения «Да, …» (press_confirm.rs) — дальше
//! та же команда ядра, что у кнопок плашки (ManualAction::SetMode / ResumeVenue / ReleaseQuarantine / ResetDailyLimit / ResetVenueLimit).
//! Кто вызывает: notify/press.rs, notify/tg_commands.rs (send_screen).
//! От чего зависит: notify/menu.rs, press_confirm.rs, core (control, manual_records), mirror_state.rs.
//! Настройки: нет.

use serde_json::Value;

use crate::core::quarantine::Quarantined;
use crate::core::risk::{Mode, Risk};
use crate::core::types::{ManualAction, ManualRecord};
use crate::storage::journal::Level;

use super::menu;
use super::mirror_state::{Pending, lock};
use super::press::Ctx;
use super::tg_buttons::{CallbackQuery, CbMessage};

/// Карантин списком по названию (как на плашке). Принимает: Risk.
pub fn quarantine(r: &Risk) -> Vec<&Quarantined> {
    let mut v: Vec<&Quarantined> = r.quarantine.values().collect();
    v.sort_by(|a, b| a.label.cmp(&b.label));
    v
}

/// Команда плашки по «c:<что>». Принимает: Risk, объект. Возвращает: ожидание (вопрос — как на плашке) или причину отказа.
pub fn control_action(r: &Risk, object: &str) -> Result<Pending, String> {
    let (what, arg) = object.split_once('.').unwrap_or((object, ""));
    let (action, question) = match what {
        "mode" => {
            let m = match arg {
                "Running" => Mode::Running,
                "Paused" => Mode::Paused,
                "Stopped" => Mode::Stopped,
                _ => return Err("неизвестный режим".into()),
            };
            if let Some(why) = menu::mode_disabled(r.mode, m) {
                return Err(why);
            }
            let a = ManualAction::SetMode { mode: m };
            let q = format!("Режим системы: «{}»?", a.title());
            (a, q)
        }
        "vr" => (
            ManualAction::ResumeVenue { venue: arg.into() },
            format!("Снять паузу площадки {arg}?"),
        ),
        "qr" => (
            ManualAction::ReleaseQuarantine { id: arg.into() },
            format!("Снять карантин монеты {arg}?"),
        ),
        "dr" => {
            let stop = if r.daily_stop {
                "\nСистема остановлена по лимиту — после сброса она останется в «Стоп», запуск — кнопкой «Старт»."
            } else {
                ""
            };
            let q = format!(
                "Сбросить лимит дня — вся система и все площадки?\nДальше лимиты считают только убытки после этой отметки; паузы площадок по убытку дня снимаются. Итог дня, история и сделки не меняются.{stop}"
            );
            (ManualAction::ResetDailyLimit, q)
        }
        // Решение 173: сброс лимита дня одной площадки — её пауза по убытку снимается, остальные не меняются.
        "dv" => (
            ManualAction::ResetVenueLimit { venue: arg.into() },
            format!(
                "Сбросить лимит дня площадки {}?\nДальше её лимит считает только убытки после отметки; пауза площадки по убытку дня снимается. Итог дня и сделки не меняются.",
                super::human::venue(arg)
            ),
        ),
        _ => return Err("неизвестная кнопка".into()),
    };
    Ok(Pending {
        key: None,
        title: action.title().into(),
        action,
        question,
    })
}

impl Ctx<'_> {
    /// Экран меню по коду (root, st, man, mode, ven, day). Возвращает: (текст, клавиатура) или None — нет такого.
    pub fn screen(&self, code: &str) -> Option<(String, Value)> {
        let r = self.core.control();
        let recs = self.core.manual_records();
        let waiting: Vec<&ManualRecord> = lock(self.state).waiting(&recs);
        Some(match code {
            "root" => menu::root(),
            "st" => menu::state(&r, waiting.len()),
            "man" => menu::manual(&waiting),
            "mode" => menu::mode(&r),
            "ven" => menu::venues(&r),
            "day" => menu::day(&r),
            _ => return None,
        })
    }

    /// «m:<экран>» — правит сообщение меню. Принимает: нажатие, сообщение, код экрана. Ошибок нет.
    pub async fn menu_screen(&self, q: &CallbackQuery, msg: &CbMessage, code: &str) {
        let Some((text, kb)) = self.screen(code) else {
            return self.answer(&q.id, Some("Неизвестный экран")).await;
        };
        self.answer(&q.id, None).await;
        let chat = msg.chat.id.to_string();
        if let Err(e) = self
            .tg
            .edit_text_markup(&chat, msg.message_id, &text, Some(kb))
            .await
        {
            // «message is not modified» — экран не изменился (нажали «Обновить»): не ошибка.
            self.log(Level::Warn, format!("Меню: экран {code} не показан: {e}"))
                .await;
        }
    }

    /// «c:<что>» — кнопка плашки режима: доступность и вопрос «Да, …». Ошибок нет.
    pub async fn menu_control(&self, q: &CallbackQuery, msg: &CbMessage, object: &str) {
        match control_action(&self.core.control(), object) {
            Ok(p) => self.ask(&q.id, Some(msg.message_id), p).await,
            Err(why) => {
                self.answer(&q.id, Some(&format!("Недоступно: {why}")))
                    .await
            }
        }
    }

    /// Отправляет экран меню новым сообщением (команды /menu, /dev, /status). Принимает: код экрана. Ошибок нет.
    pub async fn send_screen(&self, code: &str) {
        let Some((text, kb)) = self.screen(code) else {
            return;
        };
        if let Err(e) = self.tg.send_markup(self.chat, &text, Some(kb), None).await {
            self.log(Level::Error, format!("Меню не отправлено: {e}"))
                .await;
        }
    }
}
