//! Файл: venues/binance/fast_check.rs
//! Что делает: решение 425 (ревью) — сторона JSON арбитра стакана пары (fast.rs): кадр JSON depth<N>@100ms пары на
//! быстром стакане запоминается как страховка и сверяется по содержимому с быстрым стаканом SBE:
//! - совпал — пара подтверждена (время кадра пары — сейчас, живость — по JSON; book.rs rebind);
//! - не совпал один раз — ничего (кадры двух соединений приходят в разные моменты);
//! - два кадра JSON ПОДРЯД получены после последнего отданного быстрого стакана и не совпали с ним, а быстрый между
//!   ними не менялся (тот же seq) — быстрый стакан неверен (правило ревью 420/424): пара сразу на страховку этим кадром
//!   (ws_conn.rs отдаёт его ядру как раньше), изменения SBE ядру не отдаются (diverged), сессия SBE собирает стакан
//!   пары заново со снимка после паузы venue.ws_reconnect_pause_ms (sbe_pair.rs resync), счёт — строка venue.book_fast.
//!   Без этого неверный быстрый стакан при непрерывных U/u (пропуска номеров нет) держался бы, пока не изменятся
//!   именно разошедшиеся уровни. Обоснование без нового числа: изменения SBE идут раз в 20 мс, кадр JSON — раз в
//!   100 мс; молчание быстрого о реальном изменении верха дольше периода JSON — расхождение.
//!
//! Кто вызывает: ws_conn.rs (held — на каждый кадр стакана JSON).
//! От чего зависит: fast.rs (состояние), fast_sbe.rs (note), depth.rs, book.rs, gateway/feed_health.rs (BookPrint).
//! Настройки: нет своих (уровни сверки — venue.depth_levels из ws.rs Cfg).

use tokio_tungstenite::tungstenite::Utf8Bytes;

use crate::gateway::feed_health::BookPrint;

use super::book::Alive;
use super::depth;
use super::fast::{Fast, lock};

/// Итог сверки кадра JSON под замком пары.
enum Seen {
    /// Пара не на быстром стакане — кадр отдаётся ядру как раньше.
    Give,
    /// Пара на быстром стакане — кадр только страховка.
    Held,
    /// Быстрый стакан разошёлся со страховкой — пара уже на страховке, кадр отдаётся ядру, пересинхронизация.
    Diverged,
}

impl Fast {
    /// Сторона JSON: кадр пары (до разбора). Принимает: тикер, текст кадра, lastUpdateId, время получения (нс, часы
    /// бота), живость соединения JSON. Возвращает: true — пара на быстром стакане (кадр запомнен и сверен, ядру не
    /// отдаётся); false — кадр отдать ядру как раньше (в том числе при расхождении — шапка файла). Ошибок нет.
    pub fn held(&self, ticker: &str, text: &Utf8Bytes, id: i64, rx: i64, alive: &Alive) -> bool {
        let Some(i) = &self.inner else { return false };
        let seen = {
            let mut m = lock(&i.pairs);
            let Some(p) = m.get_mut(ticker) else {
                return false;
            };
            p.json = Some((text.clone(), rx, alive.clone(), id));
            if !p.fast {
                Seen::Give
            } else if p.matched == Some((id, p.seq)) {
                i.books.touch(ticker);
                Seen::Held
            } else {
                let json = top(text, i.levels);
                if json.is_some() && json == i.books.print(ticker, i.levels) {
                    p.matched = Some((id, p.seq));
                    p.miss = None;
                    i.books.rebind(ticker, alive.clone());
                    Seen::Held
                } else if json.is_none() || rx <= p.fast_rx {
                    // Не разобран или получен раньше последнего быстрого — сравнивать не с чем.
                    Seen::Held
                } else if p.miss != Some(p.seq) {
                    p.miss = Some(p.seq);
                    Seen::Held
                } else {
                    p.fast = false;
                    p.matched = None;
                    p.miss = None;
                    p.given = None;
                    p.diverged = true;
                    Seen::Diverged
                }
            }
        };
        match seen {
            Seen::Give => false,
            Seen::Held => true,
            Seen::Diverged => {
                let why = "два кадра JSON подряд новее быстрого стакана не совпали с ним, быстрый между ними не менялся";
                self.note(1, 0, 0, &format!("{ticker} — {why}"));
                if let Some(tx) = lock(&i.resync).as_ref() {
                    let _ = tx.send((ticker.to_string(), true));
                }
                false
            }
        }
    }
}

/// Верхние n уровней кадра JSON частичного стакана. None — не разобран. Ошибок нет.
fn top(text: &str, n: usize) -> Option<BookPrint> {
    let depth::Msg::Depth { book, .. } = depth::parse(text) else {
        return None;
    };
    let (bids, asks) = depth::levels(&book).ok()?;
    Some(BookPrint::top(&bids, &asks, n))
}
