//! Файл: core/types/manual_feed.rs
//! Что делает: запись экрана «Ручное управление» вместе с её кнопками (решение 156) — один источник и для экрана
//! (GET /api/manual, поле records), и для Telegram (notify/mirror*.rs): те же кнопки, те же команды ядра
//! (ManualAction), те же вопросы подтверждения и та же недоступность (disabled) с причиной.
//! Кто вызывает: core/manual_feed.rs (собирает), api/control.rs (экран), notify (Telegram).
//! От чего зависит: core/types/control.rs (ManualAction), serde.
//! Настройки: нет.

use serde::{Deserialize, Serialize};

use super::control::ManualAction;

/// Вид записи — таблица экрана «Ручное управление».
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecordKind {
    /// «Застрявшие монеты — ждут решения» (StuckAlert, решение 37).
    Alert,
    /// «Монеты «Беру на себя»» (ManualHolding).
    Holding,
    /// «Остановленные сделки — ждут разбора» (Deal в шаге Halted, решение 100).
    Halted,
    /// «USDT не вернулись — ждут перегона» (закрытая сделка, Deal.usdt_return = NotReturned, решение 163).
    UsdtReturn,
}

impl RecordKind {
    /// Буква вида в ключе записи («a», «h», «d») — короткая, чтобы ключ влез в callback_data (≤ 64 байт).
    pub fn letter(self) -> char {
        match self {
            Self::Alert => 'a',
            Self::Holding => 'h',
            Self::Halted => 'd',
            Self::UsdtReturn => 'u',
        }
    }

    /// Заголовок таблицы экрана.
    pub fn title(self) -> &'static str {
        match self {
            Self::Alert => "Застрявшая монета — ждёт решения",
            Self::Holding => "Монеты «Беру на себя»",
            Self::Halted => "Остановленная сделка — ждёт разбора",
            Self::UsdtReturn => "USDT не вернулись — ждут перегона",
        }
    }
}

/// Кнопка записи — ровно та, что на экране.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManualButton {
    /// Код кнопки внутри записи (sell_now, sell_ret, take_over, sold_man, return_bot, usdt_retry, usdt_manual) — уходит
    /// в callback_data.
    pub code: String,
    /// Надпись (как на экране).
    pub title: String,
    /// Команда ядру — та же, что шлёт экран в POST /api/manual/command. Если input задан — поле usdt
    /// у SoldManually заполняет человек (экран — prompt, Telegram — ответ на сообщение ForceReply).
    pub action: ManualAction,
    /// Вопрос подтверждения (экран — confirm, Telegram — вторая кнопка «Да, …»). «{usdt}» заменяется введённой суммой.
    pub confirm: String,
    /// Нужен ввод: вопрос «Сколько USDT вы получили…». None — без ввода.
    pub input: Option<String>,
    /// Кнопка недоступна — причина (экран — disabled с подсказкой, Telegram — кнопки нет, причина в тексте).
    pub disabled: Option<String>,
    /// Второстепенная кнопка (вид на экране, class="quiet").
    pub quiet: bool,
}

impl ManualButton {
    /// Команда с введённой суммой (для SoldManually), иначе — как есть. Принимает: сумму (None — ввода нет).
    pub fn action_with(&self, usdt: Option<&str>) -> ManualAction {
        match (&self.action, usdt) {
            (ManualAction::SoldManually { deal_id, .. }, Some(u)) => ManualAction::SoldManually {
                deal_id: deal_id.clone(),
                usdt: u.trim().to_string(),
            },
            (a, _) => a.clone(),
        }
    }

    /// Вопрос подтверждения с суммой. Принимает: сумму (None — без замены).
    pub fn confirm_with(&self, usdt: Option<&str>) -> String {
        match usdt {
            Some(u) => self.confirm.replace("{usdt}", u.trim()),
            None => self.confirm.clone(),
        }
    }
}

/// Запись экрана с кнопками.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManualRecord {
    /// Ключ записи «<буква вида>.<id>» (id алерта, записи «Беру на себя» или сделки).
    pub key: String,
    pub kind: RecordKind,
    /// id объекта: алерта, записи «Беру на себя» или сделки (для Halted и UsdtReturn).
    pub object_id: String,
    /// Сделка записи.
    pub deal_id: String,
    /// Площадка, где монеты (фильтр notify.event.venues).
    pub venue: String,
    /// Текст записи — те же поля, что в строке таблицы экрана.
    pub text: String,
    pub buttons: Vec<ManualButton>,
}

impl ManualRecord {
    /// Ключ записи. Принимает: вид, id объекта. Возвращает: «a.<id>» и т.п.
    pub fn key_of(kind: RecordKind, object_id: &str) -> String {
        format!("{}.{object_id}", kind.letter())
    }

    /// Относится ли команда к этой записи (для итога «выполнено: кто, что, итог»).
    /// Принимает: команду. Возвращает: true — команда этой записи. Ошибок нет.
    pub fn matches(&self, a: &ManualAction) -> bool {
        use ManualAction as M;
        match (self.kind, a) {
            (
                RecordKind::Alert,
                M::SellNow { alert_id } | M::SellNowReturn { alert_id } | M::TakeOver { alert_id },
            ) => alert_id == &self.object_id,
            (RecordKind::Holding, M::ReturnToBot { holding_id }) => holding_id == &self.object_id,
            (RecordKind::Halted, M::SellHalted { deal_id } | M::SellHaltedReturn { deal_id }) => {
                deal_id == &self.object_id
            }
            (
                RecordKind::UsdtReturn,
                M::UsdtRetry { deal_id } | M::UsdtReturnedManually { deal_id },
            ) => deal_id == &self.object_id,
            (_, M::SoldManually { deal_id, .. }) => deal_id == &self.deal_id,
            _ => false,
        }
    }

    /// Кнопка записи по коду. Возвращает: кнопку или None.
    pub fn button(&self, code: &str) -> Option<&ManualButton> {
        self.buttons.iter().find(|b| b.code == code)
    }
}
