//! Файл: core/deposit_warm_plan.rs
//! Что делает: решение 316 — чистая логика прогрева адресов депозита (без запросов и без ядра): какие монеты биржи
//! прогревать (направления, где биржа — B, по одному на монету), что запоминать после ответа и когда прекратить проход.
//! Память прогрева (Memo) — по ключу адаптерного кэша (тикер монеты на бирже, сеть BSC биржи):
//! - адрес получен — ключ «готов», больше не запрашивается (адрес уже в памяти адаптера до перезапуска);
//! - отказ по монете (нет адреса, требуется memo, прочий ответ биржи) — ключ «не вышло», повтор только после
//!   разблокировки/смены секретов или изменения настроек прогрева (forget_failed), а не на каждой пересборке справочника;
//! - отказ доступа или связи (секреты закрыты, ключ, IP, права, лимит запросов, таймаут, нет связи) — проход по этой
//!   бирже прекращается, ключ не запоминается (повтор — со следующим толчком).
//!
//! Кто вызывает: core/deposit_warm.rs; тест tests/core_deposit_warm_316.rs.
//! От чего зависит: core/types (Direction, VenueId), gateway/errors.rs (ErrorCode).
//! Настройки: нет (venue.deposit_warm, venue.deposit_warm_gap_ms читает deposit_warm.rs).

use std::collections::HashSet;

use crate::core::types::{Direction, VenueId};
use crate::gateway::errors::ErrorCode;

/// Ключ адреса — как у кэша адаптера: (тикер монеты на бирже, название сети BSC биржи).
pub type Key = (String, String);

/// Что делать после ответа биржи на запрос адреса.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum After {
    /// Следующая монета этой биржи.
    Next,
    /// Прекратить проход по этой бирже (доступ, связь, лимит) — повтор со следующим толчком.
    Stop,
}

/// Что делает: решает по коду ошибки адаптера — дальше по монетам или стоп прохода по бирже.
/// Принимает: код ошибки шлюза. Возвращает: After. Ошибок нет.
pub fn after(code: &ErrorCode) -> After {
    match code {
        ErrorCode::SecretsLocked
        | ErrorCode::NotConfigured
        | ErrorCode::InvalidCredentials
        | ErrorCode::InsufficientPermissions
        | ErrorCode::IpNotAllowed
        | ErrorCode::RateLimited
        | ErrorCode::Timeout
        | ErrorCode::Unreachable => After::Stop,
        _ => After::Next,
    }
}

/// Что делает: направления, где биржа `venue` — площадка продажи B (у неё нужен адрес депозита), по одному на монету
/// (тикер + адрес контракта), в порядке справочника. Принимает: направления справочника, биржу. Возвращает: список.
/// Ошибок нет.
pub fn pick(dirs: impl IntoIterator<Item = Direction>, venue: &VenueId) -> Vec<Direction> {
    let mut seen = HashSet::new();
    dirs.into_iter()
        .filter(|d| &d.sell_on == venue)
        .filter(|d| seen.insert((d.asset.ticker.clone(), d.asset.bsc_contract)))
        .collect()
}

/// Память прогрева одной биржи (живёт в задаче прогрева, до перезапуска).
#[derive(Debug, Default)]
pub struct Memo {
    ok: HashSet<Key>,
    failed: HashSet<Key>,
}

impl Memo {
    /// Что делает: нужен ли запрос по ключу (ещё не получен и не отказал). Ошибок нет.
    pub fn todo(&self, key: &Key) -> bool {
        !self.ok.contains(key) && !self.failed.contains(key)
    }

    /// Что делает: запоминает ответ биржи. Принимает: ключ, Ok(()) или код ошибки.
    /// Возвращает: After (Stop — ключ не запоминается). Ошибок нет.
    pub fn record(&mut self, key: Key, res: Result<(), &ErrorCode>) -> After {
        match res {
            Ok(()) => {
                self.failed.remove(&key);
                self.ok.insert(key);
                After::Next
            }
            Err(code) => {
                let a = after(code);
                if a == After::Next {
                    self.failed.insert(key);
                }
                a
            }
        }
    }

    /// Что делает: забывает отказы по монетам (после разблокировки секретов или смены настроек прогрева) — они будут
    /// запрошены снова. Полученные адреса остаются. Ошибок нет.
    pub fn forget_failed(&mut self) {
        self.failed.clear();
    }

    /// Сколько адресов получено и сколько монет отказали. Ошибок нет.
    pub fn counts(&self) -> (usize, usize) {
        (self.ok.len(), self.failed.len())
    }
}

/// Итог прохода по бирже (для журнала core.deposit_warm).
#[derive(Debug, Default)]
pub struct Tally {
    /// Сколько запросов адреса ушло к бирже.
    pub asked: usize,
    /// Сколько адресов получено.
    pub got: usize,
    /// Сколько отказов.
    pub failed: usize,
    /// Первая причина отказа (монета: текст ошибки адаптера; адреса в тексте нет).
    pub first_err: Option<String>,
    /// Почему проход прерван (код ошибки или «прогрев выключен»).
    pub stopped: Option<String>,
    /// Проход упёрся в закрытые секреты (SecretsLocked) — до разблокировки; разблокировка сама толкнёт прогрев.
    pub locked: bool,
}

impl Tally {
    /// Что делает: не писать журнал — запросов не было, или проход сразу упёрся в закрытые секреты (до разблокировки
    /// каждая пересборка справочника иначе давала бы строку). Ошибок нет.
    pub fn silent(&self) -> bool {
        self.asked == 0 || (self.locked && self.got == 0)
    }

    /// Что делает: строка журнала прохода. Принимает: биржу, всего адресов в памяти прогрева, отказавших монет.
    /// Возвращает: текст. Ошибок нет.
    pub fn text(&self, venue: &str, ready: usize, refused: usize) -> String {
        let mut msg = format!(
            "Прогрев адресов депозита {venue}: запрошено {}, получено {}, отказов {} (всего в памяти {ready}, отказавших монет {refused})",
            self.asked, self.got, self.failed
        );
        if let Some(e) = &self.first_err {
            msg.push_str("; первая причина — ");
            msg.push_str(e);
        }
        if let Some(st) = &self.stopped {
            msg.push_str("; проход прерван (");
            msg.push_str(st);
            msg.push_str("), повтор — со следующим толчком");
        }
        msg
    }
}
