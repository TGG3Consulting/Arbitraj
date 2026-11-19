//! Файл: venues/bybit/backup.rs
//! Что делает: решение 428 — страховка стакана Bybit в памяти. Рядом с основным потоком orderbook.{venue.depth_levels}
//! (снимок + изменения, ws_conn.rs) отдельные соединения слушают ту же пару с другой глубиной
//! orderbook.{venue.book_backup_levels} (свой локальный стакан). Здесь — чистая логика выбора «чей стакан отдать
//! ядру» по паре: основной, пока он синхронизирован; разрыв u / обрыв соединения / отказ подписки / расхождение со
//! страховкой — сразу последний стакан страховки (если он свежий и новее отданного), основной пересинхронизируется
//! (новый снимок) и после снимка снова основной. Ядру никогда не отдаётся стакан с seq не больше уже отданного (как
//! решение 109). Сверка: когда у основного и у страховки есть кадр с одним и тем же seq — первые K уровней обеих
//! сторон (K = меньшая из двух глубин) сравниваются по отпечатку; не совпали — как разрыв.
//! Кадры и потеря страховки — backup_side.rs (то же impl Hub).
//! Кто вызывает: ws_book.rs (кадры обоих потоков), ws_give.rs (обрыв соединения), market.rs (создаёт на подписку).
//! От чего зависит: backup_kind.rs (типы решений, отпечаток уровней).
//! Настройки: нет своих (глубины передаёт market.rs, срок свежести — вызывающий: venue.book_stale_ms).
//!
//! Официально (Orderbook https://bybit-exchange.github.io/docs/v5/websocket/public/orderbook; исходник
//! bybit-exchange/docs, docs/v5/websocket/public/orderbook.mdx, коммит e3936788 от 19.11.2026): «Subscribe to the
//! orderbook stream. Supports different depths.»; Spot: «Level 1 data, push frequency: 10ms», «Level 50 … 20ms»,
//! «Level 200 … 100ms», «Level 1000 … 200ms»; seq — «Cross sequence. You can use this field to compare different levels
//! orderbook data, and for the smaller seq, then it means the data is generated earlier»; «Linear, inverse, spot level
//! 1 data has snapshot message only». Что равный seq у двух глубин — одно и то же состояние книги, документация прямо
//! не пишет (только «меньше — раньше»): сверка идёт только при равном seq, проверка на живых кадрах — docs/TASKS.md.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

pub use super::backup_kind::{BackupAct, Held, Lost, Main, MainAct, Print, print};

/// Состояние пары.
#[derive(Default)]
pub(super) struct Slot {
    pub(super) main: Main,
    /// Соединение основного потока пары (для команды «переподписать пару»).
    pub(super) conn: Option<String>,
    /// seq и отпечаток последнего применённого кадра основного.
    pub(super) main_at: Option<(i64, Print)>,
    pub(super) backup: Option<Held>,
    pub(super) backup_print: Option<Print>,
    /// seq последнего отданного ядру стакана.
    pub(super) given: Option<i64>,
    /// Стакан ядра сейчас из страховки.
    pub(super) from_backup: bool,
}

/// Выбор источника стакана по парам одной подписки. `on = false` — страховки нет: основной отдаётся как раньше.
#[derive(Default)]
pub struct Hub {
    on: bool,
    /// K для сверки — меньшая из двух глубин.
    cmp: usize,
    pairs: Mutex<HashMap<String, Slot>>,
    /// Сколько сверок состоялось (равный seq, та же K) — для журнала (backup_log.rs).
    pub(super) compares: AtomicU64,
}

/// Новее ли seq отданного (нет отданного — да). Ошибок нет.
pub(super) fn newer(seq: i64, given: Option<i64>) -> bool {
    given.is_none_or(|g| seq > g)
}

impl Hub {
    /// Новый выбор. Принимает: включена ли страховка, глубины основного и страховки. Ошибок нет.
    pub fn new(on: bool, main_levels: u64, backup_levels: u64) -> Self {
        Self {
            on,
            cmp: main_levels.min(backup_levels) as usize,
            pairs: Mutex::new(HashMap::new()),
            compares: AtomicU64::new(0),
        }
    }

    /// Включена ли страховка. Ошибок нет.
    pub fn on(&self) -> bool {
        self.on
    }

    /// K сверки (сколько уровней сравнивать). Ошибок нет.
    pub fn cmp(&self) -> usize {
        self.cmp
    }

    /// Работа с состоянием пары под замком. Ошибок нет.
    pub(super) fn with<R>(&self, ticker: &str, f: impl FnOnce(&mut Slot) -> R) -> R {
        let mut m = self.pairs.lock().unwrap_or_else(|p| p.into_inner());
        if !m.contains_key(ticker) {
            m.insert(ticker.to_string(), Slot::default());
        }
        f(m.get_mut(ticker).expect("вставлено выше"))
    }

    /// Начало сессии основного соединения: пара ждёт снимок, имя соединения запомнено. Ошибок нет.
    pub fn main_session(&self, ticker: &str, conn: &str) {
        if self.on {
            self.with(ticker, |s| {
                s.main = Main::Off;
                s.conn = Some(conn.to_string());
            });
        }
    }

    /// Кадр основного применён (стакан изменился). Принимает: тикер, снимок ли, seq кадра (None — нет в кадре: отдаётся
    /// как раньше, без сверки), отпечаток, отдачу ядру (вызывается под замком пары при Give — порядок отдачи двух
    /// соединений не перемешивается). Возвращает: MainAct. Ошибок нет.
    pub fn main_frame(
        &self,
        ticker: &str,
        snapshot: bool,
        seq: Option<i64>,
        p: Print,
        give: impl FnOnce(),
    ) -> MainAct {
        let Some(seq) = seq.filter(|_| self.on) else {
            give();
            return MainAct::Give;
        };
        self.with(ticker, |s| {
            if s.main != Main::Synced {
                if !snapshot {
                    return MainAct::Skip;
                }
                s.main = Main::Synced;
            }
            s.main_at = Some((seq, p));
            if s.backup.as_ref().is_some_and(|b| b.seq == seq)
                && let Some(bp) = s.backup_print.filter(|b| b.0 == p.0)
            {
                self.compares.fetch_add(1, Ordering::Relaxed);
                if bp != p {
                    s.main = Main::Resync;
                    return MainAct::Mismatch;
                }
            }
            if !newer(seq, s.given) {
                return MainAct::Skip;
            }
            s.given = Some(seq);
            let back = std::mem::take(&mut s.from_backup);
            give();
            if back { MainAct::Back } else { MainAct::Give }
        })
    }

    /// Основной потерян (разрыв u — Resync; обрыв соединения или отказ подписки — Off; расхождение — Resync). Принимает:
    /// тикер, новое состояние, срок свежести кадра страховки (venue.book_stale_ms), действие под замком пары
    /// (Some — отдать этот стакан страховки ядру, None — стакан пары недостоверен). Возвращает: Lost. Ошибок нет.
    pub fn main_lost(
        &self,
        ticker: &str,
        state: Main,
        fresh: Duration,
        act: impl FnOnce(Option<&Held>),
    ) -> Lost {
        if !self.on {
            act(None);
            return Lost::Invalidate;
        }
        self.with(ticker, |s| {
            s.main = state;
            s.main_at = None;
            if s.from_backup {
                return Lost::Keep;
            }
            match &s.backup {
                Some(b) if b.at.elapsed() <= fresh && newer(b.seq, s.given) => {
                    s.given = Some(b.seq);
                    s.from_backup = true;
                    act(Some(b));
                    Lost::Gave
                }
                _ => {
                    act(None);
                    Lost::Invalidate
                }
            }
        })
    }

    /// Сколько раз сверка состоялась (равный seq и та же K) с начала подписки. Ошибок нет.
    pub fn compares(&self) -> u64 {
        self.compares.load(Ordering::Relaxed)
    }

    /// Источник стакана ядра — основной поток (страховка выключена — всегда да). Ошибок нет.
    pub fn main_is_source(&self, ticker: &str) -> bool {
        !self.on || self.with(ticker, |s| !s.from_backup && s.main == Main::Synced)
    }
}
