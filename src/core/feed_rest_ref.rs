//! Файл: core/feed_rest_ref.rs
//! Что делает: дополнение к решениям 202/213 (Тигран 10.11: согласовано) — «книга не менялась с переподписки» без
//! времени изменения книги по часам биржи. У бирж без changed_ms (XT, MEXC, Bitget, BingX, KuCoin … — номер или
//! содержимое) правило Unchanged решения 213 (возраст книги server_ms − changed_ms) не срабатывает никогда: тихая пара,
//! книга которой изменилась, пока подписка молчала, раз за разом получала «кадры потеряны» → круги переподписки
//! (живой случай XT 10.11). Правило: ответ REST, давший «кадры потеряны», запоминается (номер книги или первые N
//! уровней ответа) вместе с временем ответа; следующая проверка той же пары даёт тот же ответ REST, а пара с тех пор
//! переподписана (PairHealth.since_ms не раньше запомненного ответа) — книга на бирже не менялась с переподписки:
//! Unchanged (тихая, не мёртвая; без новых кругов). Ответ REST другой — книга меняется, а кадров нет — кадры потеряны,
//! обычный круг. Чистая логика без ввода-вывода (проверка — feed_confirm_tests.rs, tests/xt_book_version.rs).
//! t79 (Тигран 10.11: согласовано): у бирж по содержимому ответ, давший такой Unchanged, — стакан пары (RestBook,
//! ставит core/feed_rest_book.rs).
//! Кто вызывает: core/feed_confirm.rs (apply), core/feed_watch_act.rs, core/feed_rest_book.rs (RestBook).
//! От чего зависит: gateway/feed_health.rs (BookVersion, BookPrint, PairHealth).
//! Настройки: нет.

use crate::gateway::feed_health::{BookPrint, BookVersion, PairHealth};

/// Чем ответ REST отличается от другого ответа той же пары: номер книги (если биржа его дала) или первые N уровней
/// ответа (подтверждение по содержимому).
#[derive(Debug, Clone, PartialEq)]
pub enum RestRef {
    Id(i64),
    Print(BookPrint),
}

impl RestRef {
    /// Отпечаток ответа REST. Принимает: версию книги. Возвращает: номер, иначе уровни ответа; None — сравнивать нечем.
    /// Ошибок нет.
    pub fn of(v: &BookVersion) -> Option<RestRef> {
        match (v.id, &v.print) {
            (Some(id), _) => Some(RestRef::Id(id)),
            (None, Some(p)) => Some(RestRef::Print(p.rest.clone())),
            (None, None) => None,
        }
    }
}

/// Ответ REST, давший «кадры потеряны»: отпечаток и когда получен (часы бота).
#[derive(Debug, Clone, PartialEq)]
pub struct LostRef {
    pub rest: RestRef,
    pub at_ms: i64,
}

/// Книга на бирже не менялась с переподписки: тот же ответ REST, что дал «кадры потеряны», и пара переподписана после
/// него. Принимает: запомненный ответ, отпечаток нового ответа, пару из свежего снимка feed_health. Возвращает: true —
/// не менялась. Ошибок нет.
pub fn unchanged_since_resub(
    prev: Option<&LostRef>,
    now: Option<&RestRef>,
    p: &PairHealth,
) -> bool {
    match (prev, now) {
        (Some(prev), Some(now)) => prev.rest == *now && p.since_ms >= prev.at_ms,
        _ => false,
    }
}

/// t79 (Тигран 10.11: согласовано): ответ REST, по которому пара признана «не менялась с переподписки» (Unchanged
/// по unchanged_since_resub), — стакан пары (core/feed_rest_book.rs, контракт book_from_rest). Только у бирж ПО
/// СОДЕРЖИМОМУ: уровни — первые N уровней ЭТОГО ответа (N = venue.depth_levels, столько же даёт кадр потока), они же
/// равны запомненному ответу «кадры потеряны» (иначе Unchanged не было бы). У номера (RestRef::Id) уровней нет — None.
#[derive(Debug, Clone, PartialEq)]
pub struct RestBook {
    /// Первые N уровней ответа (от лучшей цены).
    pub levels: BookPrint,
    /// Когда запрос отправлен (часы бота): кадр пары после этого момента главнее снимка.
    pub sent_ms: i64,
    /// Когда получен ЭТОТ ответ (часы бота): время стакана — от него считается venue.book_stale_ms.
    pub recv_ms: i64,
}

impl RestBook {
    /// Стакан из отпечатка ответа. Принимает: отпечаток (None — нечем), время отправки и получения ответа.
    /// Возвращает: Some — только для уровней (RestRef::Print). Ошибок нет.
    pub fn of(rest: Option<&RestRef>, sent_ms: i64, recv_ms: i64) -> Option<RestBook> {
        match rest {
            Some(RestRef::Print(p)) => Some(RestBook {
                levels: p.clone(),
                sent_ms,
                recv_ms,
            }),
            _ => None,
        }
    }
}
