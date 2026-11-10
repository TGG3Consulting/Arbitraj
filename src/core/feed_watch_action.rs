//! Файл: core/feed_watch_action.rs
//! Что делает: решения 202 и 213 — действия сторожа пар, которые выдаёт чистая логика (feed_watch_logic.rs,
//! feed_watch_conn.rs, feed_confirm.rs) и выполняет core/feed_watch.rs (команды адаптеру, журнал, Telegram).
//! Кто вызывает: feed_watch_logic.rs (pub use), feed_watch_conn.rs, feed_confirm.rs, feed_watch_act.rs, тесты.
//! От чего зависит: feed_rest_ref.rs (RestBook).
//! Настройки: нет.

/// Что сделать.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Переподписать пару: попытка в круге (с 1) и номер круга (с 1).
    Resub {
        conn: String,
        ticker: String,
        attempt: u32,
        round: u32,
    },
    /// Пара ожила после попыток.
    Recovered { conn: String, ticker: String },
    /// Переподключить соединение: мёртвых пар из всех.
    Reconnect {
        conn: String,
        dead: usize,
        total: usize,
    },
    /// Переподключить нельзя — по паре соединения идёт сделка (id сделок).
    Blocked {
        conn: String,
        dead: usize,
        total: usize,
        deals: Vec<String>,
    },
    /// Предел переподключений за час исчерпан — нужен человек.
    GiveUp {
        conn: String,
        dead: usize,
        total: usize,
        count: usize,
    },
    /// Решение 213: спросить у биржи версию стакана пары (book_version), ответ — feed_confirm::apply.
    Check { conn: String, ticker: String },
    /// Решение 213: книга не менялась с последнего кадра — стакан свежий до until_ms (book_confirmed); log — сказать
    /// в журнал (состояние пары сменилось).
    Confirmed {
        conn: String,
        ticker: String,
        until_ms: i64,
        log: bool,
    },
    /// Решение 213: книга менялась до (пере)подписки и с тех пор не менялась — переподписка не поможет, стакана этой
    /// версии нет; log — состояние пары сменилось (дополнение 10.11: действие — на каждый ответ, журнал — по log и
    /// venue.feed_watch_log_ms, feed_watch_log.rs). t79: book — уровни ответа REST, по которому так решено (только у
    /// бирж по содержимому), — ставятся стаканом пары (feed_rest_book.rs); None — стакана этой версии нет.
    Unchanged {
        conn: String,
        ticker: String,
        book: Option<super::feed_rest_ref::RestBook>,
        log: bool,
    },
    /// Решение 213: на бирже книга новее последнего кадра — кадры потеряны; log — как у Unchanged.
    Lost {
        conn: String,
        ticker: String,
        why: String,
        log: bool,
    },
    /// Решение 213: подтвердить не удалось (ошибка REST или нет сравнимой версии); log — причина сменилась.
    CheckFailed {
        conn: String,
        ticker: String,
        why: String,
        log: bool,
    },
    /// Решение 213: пары молчат, подтвердить нельзя (why), кадры не по расписанию — только журнал, по паре один раз.
    QuietNoConfirm {
        conn: String,
        why: String,
        tickers: Vec<String>,
    },
}
