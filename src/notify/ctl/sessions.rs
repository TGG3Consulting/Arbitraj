//! Файл: notify/ctl/sessions.rs
//! Что делает: память экранов чата «Управление» (решение 176) — только в памяти процесса: открытые экраны (сообщение,
//! что на нём, когда трогали — для лайв-обновления), денежные кнопки текущей отрисовки (продажа монеты, шаги перевода
//! USDT) по короткому номеру, вопросы суммы (ForceReply). Каждая отрисовка с денежными кнопками получает НОВЫЙ номер
//! (прежний забывается): нажатие по устаревшей отрисовке — «экран обновился / устарел», деньги по нему не двигаются.
//! Номера начинаются с времени запуска процесса в мс и только растут — после перезапуска номер старой кнопки не
//! совпадёт ни с одним новым («экран устарел, откройте меню»). Срок экрана и диалога — notify.dialog_ttl_s.
//! Кто вызывает: notify/ctl/*.rs, notify/updates*.rs (общая память на все поллеры).
//! От чего зависит: core/usdt_transfer_plan.rs (End), rust_decimal.
//! Настройки: notify.dialog_ttl_s (передаёт вызывающий).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use rust_decimal::Decimal;

use crate::core::usdt_transfer_check::End;

/// Что на экране.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum View {
    Root,
    State,
    Balances,
    /// Балансы биржи по id.
    Venue(String),
    /// Балансы кошелька DEX по id.
    Wallet(String),
    Manual,
    /// Решение 192: карточка проблемной сделки — позиция в списке ждущих и ключ записи.
    Card(usize, String),
    /// Диалог перевода USDT.
    Transfer,
}

impl View {
    /// Обновляется ли экран сам (лайв): состояние и балансы. Ошибок нет.
    pub fn live(&self) -> bool {
        matches!(self, View::State | View::Venue(_) | View::Wallet(_))
    }
}

/// Шаг диалога перевода.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    From,
    To,
    Amount,
    Summary,
}

/// Диалог «Перевести USDT»: что выбрано.
#[derive(Debug, Clone)]
pub struct Dialog {
    pub step: Step,
    pub from: Option<End>,
    pub to: Option<End>,
    /// Можно вывести (из сводки на шаге «Сколько»).
    pub withdrawable: Option<Decimal>,
    pub amount: Option<Decimal>,
    /// Последняя причина отказа — показывается на экране шага.
    pub error: Option<String>,
}

/// Денежная кнопка текущей отрисовки.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ref {
    /// «Продать» монету со счёта биржи (решение 174).
    Sell {
        venue: String,
        account: String,
        asset: String,
        free: Decimal,
    },
    /// Площадка перевода (шаги «Откуда» / «Куда»).
    End(End),
    /// Доля «можно вывести» (notify.transfer_pct), %.
    Pct(Decimal),
}

/// Экран: сообщение и его отрисовка.
#[derive(Debug, Clone)]
pub struct Sess {
    pub chat: i64,
    pub msg: i64,
    pub view: View,
    pub refs: Vec<Ref>,
    pub dialog: Option<Dialog>,
    /// Последнее действие человека на экране (лайв — пока не старше notify.live_idle_s).
    pub touched: Instant,
    /// Последний показанный текст (лайв правит, только если изменилось).
    pub shown: String,
    /// Экран заморожен (бездействие) — до нажатия «Обновить».
    pub frozen: bool,
    /// Когда лайв правил экран (не чаще notify.live_refresh_s); None — не правил.
    pub edited: Option<Instant>,
}

/// Память экранов.
#[derive(Debug)]
pub struct CtlState {
    next: u64,
    /// Номер отрисовки → экран.
    pub sess: HashMap<u64, Sess>,
    /// Вопрос суммы: (чат, сообщение с ForceReply) → номер отрисовки диалога.
    pub prompts: HashMap<(i64, i64), u64>,
    /// Telegram велел подождать (retry_after) — лайв не правит до этого момента.
    pub pause_until: Option<Instant>,
}

impl Default for CtlState {
    /// Пустая память; номера — с времени запуска в мс. Ошибок нет.
    fn default() -> Self {
        Self {
            next: super::super::mirror_state::start_seed(),
            sess: HashMap::new(),
            prompts: HashMap::new(),
            pause_until: None,
        }
    }
}

/// Общая память (дёшево клонируется).
pub type Shared = Arc<Mutex<CtlState>>;

/// Замок памяти (отравленный — берём как есть).
pub fn lock(s: &Shared) -> MutexGuard<'_, CtlState> {
    s.lock().unwrap_or_else(|p| p.into_inner())
}

impl CtlState {
    /// Новый номер отрисовки (callback_data кнопок собирается до отправки или правки). Ошибок нет.
    pub fn reserve(&mut self) -> u64 {
        self.next += 1;
        self.next
    }

    /// Кладёт отрисовку экрана под номером reserve: прежняя отрисовка того же сообщения забывается (её кнопки —
    /// «экран обновился»); в том же чате остальные экраны перестают обновляться сами (лайв — только у последнего,
    /// лимиты Telegram). Принимает: номер, экран. Ошибок нет.
    pub fn put(&mut self, id: u64, s: Sess) {
        self.sess
            .retain(|_, x| !(x.chat == s.chat && x.msg == s.msg));
        for x in self.sess.values_mut().filter(|x| x.chat == s.chat) {
            x.frozen = true;
        }
        self.sess.insert(id, s);
    }

    /// Экран по номеру, если не старше срока (денежные кнопки и шаги диалога — только так).
    /// Принимает: номер, срок notify.dialog_ttl_s. Возвращает: экран или None — устарел, неизвестен (в т.ч. после
    /// перезапуска). Ошибок нет.
    pub fn get(&mut self, id: u64, ttl: Duration) -> Option<Sess> {
        self.purge(ttl);
        self.sess.get(&id).cloned()
    }

    /// Забывает экраны старше срока и их вопросы суммы. Принимает: срок. Ошибок нет.
    pub fn purge(&mut self, ttl: Duration) {
        self.sess.retain(|_, s| s.touched.elapsed() <= ttl);
        let alive: Vec<u64> = self.sess.keys().copied().collect();
        self.prompts.retain(|_, id| alive.contains(id));
    }
}
