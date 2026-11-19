//! Файл: venues/binance/fast.rs
//! Что делает: решение 425 (общая постановка решений 420–424) — какой стакан пары Binance отдаётся ядру. Пара
//! соединений одной пачки монет: быстрое SBE (изменения <symbol>@depth каждые 20 мс → локальный стакан, sbe_pair.rs) и
//! нынешнее JSON <symbol>@depth<N>@100ms (ws_conn.rs) — страховка, слушается всегда параллельно. Пока быстрый стакан
//! пары синхронизирован — ядру идёт он, кадры JSON только запоминаются (последний) и сверяются по содержимому.
//! Пропуск номеров, сбой снимка, обрыв SBE → пара сразу на страховку: последний кадр JSON, если он получен ПОСЛЕ
//! последнего отданного быстрого стакана, — ядру сейчас; иначе стакан пары снимается (ядро знает «недостоверно») и
//! следующий кадр JSON (≤ 100 мс) отдаётся как раньше. После пересинхронизации — снова быстрый.
//! Здесь — состояние и сторона JSON (кроме сверки кадра — fast_check.rs); сторона SBE и счётчики — fast_sbe.rs.
//! Кто вызывает: ws_conn.rs и ws.rs (сторона JSON), sbe_run.rs (создаёт), sbe_conn.rs (Pairs), fast_sbe.rs, fast_check.rs.
//! От чего зависит: book.rs (Books), gateway/core_out_book.rs.
//! Настройки: нет своих (уровни ядру — venue.depth_levels из ws.rs Cfg).
//!
//! Почему не по номеру (п.3–4 постановки): связь lastUpdateId частичного стакана JSON с номерами потока изменений
//! (U/u) в документации не описана (решение «202/213 (Binance)», book_print.rs), времени биржи в частичном стакане нет.
//! Поэтому «не старее уже отданного» — по времени получения на нашем сервере (now_ns, одни часы у обеих сторон), а
//! сверка — только по содержимому: верхние N уровней кадра JSON равны верхним N быстрого стакана — пара подтверждена
//! (время кадра пары — сейчас, живость стакана — по соединению JSON, book.rs rebind: тихая пара без изменений SBE не
//! устаревает, пока страховка подтверждает её содержимое); не равны — ничего (кадры двух соединений приходят в разные
//! моменты, одно расхождение само по себе не доказывает пропуск). Ревью 425 (правило ревью 420/424): два кадра JSON
//! ПОДРЯД получены после последнего быстрого стакана и не совпали с ним, а быстрый между ними не менялся — быстрый
//! неверен: пара на страховку этим кадром и пересинхронизация (fast_check.rs). Счётчики — fast_sbe.rs.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Utf8Bytes;

use crate::gateway::core_out_book::{BookOut, Fed};
use crate::gateway::events::MarketEvent;

use super::book::{Alive, Books};
use super::ctx::Ctx;

/// Арбитр пачки монет. Default — SBE выключен: всё как раньше, без замков.
#[derive(Clone, Default)]
pub struct Fast {
    pub(super) inner: Option<Arc<Inner>>,
}

pub(super) struct Inner {
    pub(super) ctx: Ctx,
    pub(super) conn: String,
    pub(super) books: Books,
    pub(super) out: mpsc::Sender<MarketEvent>,
    pub(super) kept: BookOut,
    pub(super) levels: usize,
    pub(super) pairs: Mutex<HashMap<String, Pair>>,
    /// Канал пересинхронизации пар в сессию SBE: (тикер, после паузы venue.ws_reconnect_pause_ms). Без паузы — команда
    /// сторожа ядра «переподписать пару»; с паузой — быстрый стакан разошёлся со страховкой (fast_check.rs).
    pub(super) resync: Mutex<Option<mpsc::UnboundedSender<(String, bool)>>>,
    pub(super) stats: Mutex<Stats>,
}

/// Состояние пары.
#[derive(Default)]
pub(super) struct Pair {
    /// Ядру идёт быстрый стакан.
    pub(super) fast: bool,
    /// Когда отдан последний быстрый стакан (нс, часы бота).
    pub(super) fast_rx: i64,
    /// Номер отданного быстрого стакана (растёт).
    pub(super) seq: u64,
    /// Последний кадр JSON: текст, время получения, живость его соединения, lastUpdateId.
    pub(super) json: Option<(Utf8Bytes, i64, Alive, i64)>,
    /// Кадр JSON (lastUpdateId), совпавший по содержимому с быстрым стаканом номер seq.
    pub(super) matched: Option<(i64, u64)>,
    /// lastUpdateId кадра JSON, уже отданного ядру при переходе на страховку (тот же кадр второй раз не отдаётся).
    pub(super) given: Option<i64>,
    /// Ревью 425: номер быстрого стакана (seq), с которым не совпал кадр JSON, полученный после него.
    pub(super) miss: Option<u64>,
    /// Ревью 425: быстрый стакан разошёлся со страховкой — изменения SBE ядру не отдаются, пока пара не сброшена
    /// (fast_lost после пересинхронизации).
    pub(super) diverged: bool,
}

/// Счётчики соединения и метка строки журнала.
#[derive(Default)]
pub(super) struct Stats {
    pub(super) token: Option<u64>,
    pub(super) to_json: i64,
    pub(super) to_fast: i64,
    pub(super) resyncs: i64,
}

/// Замок без паники на отравлении. Ошибок нет.
pub(super) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl Fast {
    /// Что делает: арбитр пачки. Принимает: окружение, имя соединения, тикеры пачки, стаканы, канал ядра, уровней ядру.
    /// Ошибок нет.
    pub fn new(
        ctx: &Ctx,
        conn: &str,
        tickers: &[String],
        books: &Books,
        out: &mpsc::Sender<MarketEvent>,
        levels: usize,
    ) -> Self {
        let pairs = tickers
            .iter()
            .map(|t| (t.clone(), Pair::default()))
            .collect();
        Fast {
            inner: Some(Arc::new(Inner {
                kept: BookOut::new(out, ctx.venue(), &ctx.settings, &ctx.journal),
                ctx: ctx.clone(),
                conn: conn.to_string(),
                books: books.clone(),
                out: out.clone(),
                levels,
                pairs: Mutex::new(pairs),
                resync: Mutex::new(None),
                stats: Mutex::new(Stats::default()),
            })),
        }
    }

    /// Сторона JSON: отдать стакан из кадра — под замком пары. Принимает: тикер, lastUpdateId, отдача (books.put +
    /// ядру). Возвращает: None — пара уже на быстром стакане или этот кадр уже отдан при переходе (не отдан). Ошибок нет.
    pub fn give_json(&self, ticker: &str, id: i64, f: impl FnOnce() -> Fed) -> Option<Fed> {
        let Some(i) = &self.inner else {
            return Some(f());
        };
        let mut m = lock(&i.pairs);
        let Some(p) = m.get_mut(ticker) else {
            return Some(f());
        };
        if p.fast || p.given.take() == Some(id) {
            return None;
        }
        Some(f())
    }

    /// Сторона JSON: стакан пары из JSON недостоверен (отказ подписки, кадр не разобран). Принимает: тикер, снятие
    /// стакана (ws.rs invalidate) — выполняется под замком пары, если пара не на быстром стакане (как раньше; под
    /// замком — чтобы быстрый стакан, отданный в этот момент, не был снят). Возвращает: true — снят. Ошибок нет.
    pub fn json_lost(&self, ticker: &str, clear: impl FnOnce()) -> bool {
        let Some(i) = &self.inner else {
            clear();
            return true;
        };
        let mut m = lock(&i.pairs);
        let off = m.get_mut(ticker).is_none_or(|p| {
            p.json = None;
            !p.fast
        });
        if off {
            clear();
        }
        off
    }

    /// Сторона JSON: сессия JSON закончилась — страховки нет, запомненные кадры забыты. Принимает: тикер, снятие
    /// стакана — выполняется под замком пары, если пара не на быстром стакане (его не снимать). Ошибок нет.
    pub fn json_down(&self, ticker: &str, clear: impl FnOnce()) {
        self.json_lost(ticker, clear);
    }

    /// Сторона JSON: сторож ядра переподписывает пару — быстрый стакан пары тоже собрать заново со снимка. Ошибок нет.
    pub fn resync(&self, ticker: &str) {
        if let Some(i) = &self.inner
            && let Some(tx) = lock(&i.resync).as_ref()
        {
            let _ = tx.send((ticker.to_string(), false));
        }
    }
}
