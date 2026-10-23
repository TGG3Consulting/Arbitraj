//! Файл: core/wait.rs
//! Что делает: паузы фоновых задач ядра по настройке. Настройка задана — ждём столько мс или секунд,
//! сколько в ней (или раньше, если ядро «толкнуло» задачу). Не задана — честно пишем в журнал
//! один раз и ждём, пока Тигран её задаст: никаких значений по умолчанию.
//! Кто вызывает: core/registry_sync.rs, позже ledger.rs, usdt_return.rs, transfers.rs.
//! От чего зависит: core/deps.rs, settings (read), tokio.
//! Настройки: любые интервалы (имя передаёт вызывающий).

use std::time::Duration;

use tokio::sync::Notify;

use crate::settings::read::At;
use crate::storage::journal::Level;

use super::deps::Deps;

/// В каких единицах настройка интервала.
#[derive(Debug, Clone, Copy)]
pub enum Unit {
    Ms,
    Seconds,
}

/// Пауза фоновой задачи. Помнит последнюю проблему, чтобы не писать её в журнал на каждом круге.
pub struct Pacer {
    /// Имя настройки интервала.
    name: &'static str,
    unit: Unit,
    /// Площадка области настройки (None — вся система).
    venue: Option<String>,
    /// Последнее сообщение «не задано», уже записанное в журнал.
    reported: Option<String>,
}

impl Pacer {
    /// Создаёт паузу. Принимает: имя настройки, её единицу, площадку области. Ошибок нет.
    pub fn new(name: &'static str, unit: Unit, venue: Option<String>) -> Self {
        Self {
            name,
            unit,
            venue,
            reported: None,
        }
    }

    /// Ждёт следующего круга.
    /// Настройка задана — спит её значение или до `kick`. Не задана — один раз пишет в журнал
    /// и ждёт изменения этой настройки (или `kick`). Ошибок не возвращает.
    pub async fn pause(&mut self, deps: &Deps, kick: &Notify) {
        let at = At {
            venue: self.venue.as_deref(),
            coin: None,
        };
        match deps.settings.integer(self.name, at) {
            Ok(v) => {
                self.reported = None;
                let d = match self.unit {
                    Unit::Ms => Duration::from_millis(v),
                    Unit::Seconds => Duration::from_secs(v),
                };
                tokio::select! {
                    _ = tokio::time::sleep(d) => {}
                    _ = kick.notified() => {}
                }
            }
            Err(e) => {
                if self.reported.as_deref() != Some(e.message.as_str()) {
                    deps.log(
                        Level::Warn,
                        "core.not_configured",
                        self.venue.as_deref(),
                        e.message.clone(),
                        None,
                    )
                    .await;
                    self.reported = Some(e.message);
                }
                let mut w = deps.watch(&[self.name]);
                tokio::select! {
                    _ = w.next() => {}
                    _ = kick.notified() => {}
                }
            }
        }
    }
}
