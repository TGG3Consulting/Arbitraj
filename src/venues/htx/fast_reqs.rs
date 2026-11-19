//! Файл: venues/htx/fast_reqs.rs
//! Что делает: решение 422 — очередь запросов снимка {"req": "market.$symbol.mbp.$levels"} одного соединения быстрого
//! канала HTX: символ стоит в очереди не больше одного раза (в очереди или ждёт ответа); у каждого — «не раньше»
//! (после разрыва — пауза venue.ws_reconnect_pause_ms, урок 12); темп отправки — venue.fast_book_req_per_sec
//! (fast_conn.rs; официально «The limitation of single connection is 100 ms»). Пары, чью подписку биржа не приняла,
//! запросов не получают до конца сессии.
//! Кто вызывает: fast_pair.rs, fast_conn.rs.
//! От чего зависит: tokio (Instant).
//! Настройки: нет (паузу и темп передаёт вызывающий).

use std::collections::{HashSet, VecDeque};
use std::time::Duration;

use tokio::time::Instant;

/// Очередь запросов снимка соединения на одну сессию.
#[derive(Default)]
pub struct Reqs {
    /// (не раньше, символ).
    queue: VecDeque<(Instant, String)>,
    /// Символы в очереди или ждущие ответа на запрос.
    queued: HashSet<String>,
    /// Символы, чью подписку биржа не приняла (до конца сессии).
    off: HashSet<String>,
}

impl Reqs {
    /// Есть ли что слать (в том числе позже). Ошибок нет.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Ставит запрос символа (уже стоит или ждёт ответа, подписка не принята — ничего). Принимает: символ, через
    /// сколько можно слать. Ошибок нет.
    pub fn queue(&mut self, symbol: &str, after: Duration) {
        if self.off.contains(symbol) || !self.queued.insert(symbol.to_string()) {
            return;
        }
        self.queue
            .push_back((Instant::now() + after, symbol.to_string()));
    }

    /// Стоит ли символ в очереди или ждёт ответа. Ошибок нет.
    pub fn queued(&self, symbol: &str) -> bool {
        self.queued.contains(symbol)
    }

    /// Ответ на запрос пришёл (снимок или ошибка). Ошибок нет.
    pub fn answered(&mut self, symbol: &str) {
        self.queued.remove(symbol);
    }

    /// Забывает запрос символа (в очереди и ждущий ответа) — неразборчивый кадр мог быть самим ответом. Ошибок нет.
    pub fn forget(&mut self, symbol: &str) {
        self.queued.remove(symbol);
        self.queue.retain(|(_, s)| s != symbol);
    }

    /// Первый символ, которому уже можно слать (убирается из очереди, остаётся «ждёт ответа»). Ошибок нет.
    pub fn due(&mut self) -> Option<String> {
        let now = Instant::now();
        let i = self.queue.iter().position(|(at, _)| *at <= now)?;
        self.queue.remove(i).map(|(_, s)| s)
    }

    /// Подписку символа биржа не приняла: запросов больше нет. Ошибок нет.
    pub fn turn_off(&mut self, symbol: &str) {
        self.forget(symbol);
        self.off.insert(symbol.to_string());
    }

    /// Подписка символа не принята. Ошибок нет.
    pub fn is_off(&self, symbol: &str) -> bool {
        self.off.contains(symbol)
    }
}
