//! Файл: venues/binance/fast_sbe.rs
//! Что делает: решение 425 — сторона SBE арбитра стакана пары (fast.rs): быстрый стакан изменился — ядру (пара на
//! быстром стакане); быстрый стакан недостоверен — пара сразу на страховку JSON (последний кадр JSON, если он получен
//! после последнего отданного быстрого стакана, иначе стакан снимается до следующего кадра JSON); канал
//! пересинхронизации пар сессии SBE; счётчики переключений и пересинхронизаций — одна повторяющаяся строка журнала
//! venue.book_fast на соединение (решения 155/354, storage/journal_counted.rs — без ожидания базы).
//! Кто вызывает: sbe_pair.rs, sbe_conn.rs, sbe_run.rs, fast_check.rs (note).
//! От чего зависит: fast.rs (состояние), depth.rs (разбор запомненного кадра JSON), ws.rs (invalidate),
//! gateway/core_out_book.rs, storage/journal_counted.rs.
//! Настройки: нет.

use tokio::sync::mpsc;

use crate::core::types::{Asset, OrderBook};
use crate::gateway::core_out_book::Fed;
use crate::gateway::tap::now_ns;
use crate::storage::journal::Level;

use super::book::Alive;
use super::ctx::ID;
use super::depth;
use super::fast::{Fast, lock};

impl Fast {
    /// Сторона SBE: быстрый стакан пары изменился — ядру. Принимает: стакан, живость соединения SBE.
    /// Возвращает: итог отдачи. Ошибок нет.
    pub fn fast_book(&self, ob: OrderBook, alive: &Alive) -> Fed {
        let Some(i) = &self.inner else {
            return Fed::Lost;
        };
        let ticker = ob.asset.ticker.clone();
        let (fed, first) = {
            let mut m = lock(&i.pairs);
            let Some(p) = m.get_mut(&ticker) else {
                return Fed::Lost;
            };
            // Ревью 425: быстрый разошёлся со страховкой (fast_check.rs) — до сброса пары (fast_lost) ядру не отдаётся.
            if p.diverged {
                return Fed::Sent;
            }
            let first = !p.fast;
            p.fast = true;
            p.seq += 1;
            p.matched = None;
            p.given = None;
            p.fast_rx = now_ns();
            i.books.put(ob.clone(), alive.clone());
            (i.kept.book(&i.out, ob), first)
        };
        if first {
            self.note(0, 1, 0, &format!("{ticker} — снова быстрый стакан"));
        }
        fed
    }

    /// Сторона SBE: быстрый стакан пары недостоверен — на страховку (шапка файла). Принимает: монету, причину,
    /// пересинхронизация ли это (счётчик). Ошибок нет.
    pub fn fast_lost(&self, asset: &Asset, why: &str, resync: bool) {
        let Some(i) = &self.inner else { return };
        let moved = {
            let mut m = lock(&i.pairs);
            let Some(p) = m.get_mut(&asset.ticker) else {
                return;
            };
            // Пара сброшена (сессия SBE пересобирает стакан со снимка) — запрет отдачи после расхождения снят.
            p.diverged = false;
            p.miss = None;
            let moved = p.fast;
            if moved {
                p.fast = false;
                p.matched = None;
                match &p.json {
                    Some((text, rx, alive, id)) if *rx > p.fast_rx => match book(text, asset) {
                        Some(ob) => {
                            i.books.put(ob.clone(), alive.clone());
                            let _ = i.kept.book(&i.out, ob);
                            p.given = Some(*id);
                        }
                        None => super::ws::invalidate(&i.ctx, &i.books, &i.out, &asset.ticker),
                    },
                    _ => super::ws::invalidate(&i.ctx, &i.books, &i.out, &asset.ticker),
                }
            }
            moved
        };
        if moved || resync {
            self.note(
                u64::from(moved),
                0,
                u64::from(resync),
                &format!("{} — {why}", asset.ticker),
            );
        }
    }

    /// Сторона SBE: канал пересинхронизации пар сессии SBE (None — сессии нет). Ошибок нет.
    pub fn set_resync(&self, tx: Option<mpsc::UnboundedSender<(String, bool)>>) {
        if let Some(i) = &self.inner {
            *lock(&i.resync) = tx;
        }
    }

    /// Счётчики и строка журнала соединения (одна, повторяющаяся). Принимает: прибавки (на страховку, на быстрый,
    /// пересинхронизаций), последнее событие. Вызывать не под замком пар. Ошибок нет.
    pub(super) fn note(&self, to_json: u64, to_fast: u64, resyncs: u64, last: &str) {
        let Some(i) = &self.inner else { return };
        let mut s = lock(&i.stats);
        s.to_json += to_json as i64;
        s.to_fast += to_fast as i64;
        s.resyncs += resyncs as i64;
        let total = s.to_json + s.to_fast + s.resyncs;
        let msg = format!(
            "Binance {}: стакан SBE → страховка JSON {} раз, обратно на SBE {}, пересинхронизаций {}; последнее: {last}",
            i.conn, s.to_json, s.to_fast, s.resyncs
        );
        let data =
            serde_json::json!({ "to_json": s.to_json, "to_fast": s.to_fast, "resyncs": s.resyncs });
        match s.token {
            Some(t) => i
                .ctx
                .journal
                .update_queued(t, total, chrono::Utc::now(), msg, Some(data)),
            None => {
                s.token = i.ctx.journal.counted_queued(
                    Level::Warn,
                    "venue.book_fast",
                    Some(ID),
                    msg,
                    Some(data),
                );
            }
        }
    }
}

/// Стакан ядра из кадра JSON. None — не разобран. Ошибок нет.
fn book(text: &str, asset: &Asset) -> Option<OrderBook> {
    let depth::Msg::Depth { book, .. } = depth::parse(text) else {
        return None;
    };
    let (bids, asks) = depth::levels(&book).ok()?;
    Some(depth::to_book(asset.clone(), bids, asks))
}
