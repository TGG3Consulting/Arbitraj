//! Файл: gateway/feed_reg.rs
//! Что делает: решение 202 — общий для адаптеров учёт свежести потока стаканов (FeedReg): соединение регистрирует свои
//! пары, на каждом кадре отмечает время, получает команды сторожа ядра из своего канала; снимок свежести и доставка
//! команд для методов контракта feed_health / resubscribe_pair / reconnect_conn. Решение 213: на кадре — ещё и версия
//! книги из кадра (номер и/или время изменения по часам биржи), чтобы ядро сравнило её с версией по REST.
//! Кто вызывает: адаптеры бирж (соединения стаканов и методы контракта).
//! От чего зависит: gateway/feed_health.rs (типы), gateway/errors.rs, tokio (канал команд), chrono.
//! Настройки: нет.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, RwLock};

use tokio::sync::mpsc;

use super::feed_health::{Cmd, ConnHealth, FeedHealth, PairHealth};

/// Сейчас, unix мс.
fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// «Не было» для версии книги (номер может быть любым числом, поэтому не 0).
const NO_VER: i64 = i64::MIN;

/// Учёт одной пары. Время: 0 — «не было»; версия: NO_VER — «не было».
struct Pair {
    ticker: String,
    /// Последний кадр стакана (часы бота).
    last: AtomicI64,
    /// (Пере)подписка — начало прогрева.
    since: AtomicI64,
    /// Решение 213: номер версии книги из последнего кадра.
    id: AtomicI64,
    /// Решение 213: время изменения книги из последнего кадра (часы биржи).
    changed: AtomicI64,
}

/// Учёт одного соединения: время кадров, пары, канал команд. 0 в атомике — «не было».
struct Conn {
    name: String,
    alive: AtomicI64,
    started: AtomicI64,
    pairs: Vec<Pair>,
    tx: mpsc::UnboundedSender<Cmd>,
}

/// Учёт свежести соединений адаптера. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct FeedReg {
    conns: Arc<RwLock<Vec<Arc<Conn>>>>,
}

/// Ручка соединения: отметки времени без поиска по всем соединениям. Дешёво клонируется.
#[derive(Clone)]
pub struct ConnHandle {
    conn: Arc<Conn>,
    idx: Arc<HashMap<String, usize>>,
}

impl FeedReg {
    /// Новая подписка адаптера: прежние соединения забыты (их задачи остановлены). Ошибок нет.
    pub fn clear(&self) {
        self.conns
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    /// Регистрирует соединение и его пары (тикеры площадки, решение 211). Возвращает: ручку и канал команд соединения.
    pub fn register(
        &self,
        name: &str,
        tickers: &[String],
    ) -> (ConnHandle, mpsc::UnboundedReceiver<Cmd>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let conn = Arc::new(Conn {
            name: name.to_string(),
            alive: AtomicI64::new(0),
            started: AtomicI64::new(0),
            pairs: tickers
                .iter()
                .map(|t| Pair {
                    ticker: t.clone(),
                    last: AtomicI64::new(0),
                    since: AtomicI64::new(0),
                    id: AtomicI64::new(NO_VER),
                    changed: AtomicI64::new(NO_VER),
                })
                .collect(),
            tx,
        });
        let idx = tickers
            .iter()
            .enumerate()
            .map(|(i, t)| (t.clone(), i))
            .collect();
        self.conns
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .push(conn.clone());
        (
            ConnHandle {
                conn,
                idx: Arc::new(idx),
            },
            rx,
        )
    }

    /// Снимок свежести. Принимает: признаки площадки (по расписанию, переподписка пары, подтверждение по REST —
    /// решение 213). Ошибок нет.
    pub fn health(&self, scheduled: bool, per_pair: bool, confirm: bool) -> FeedHealth {
        let opt = |v: i64| (v > 0).then_some(v);
        let ver = |a: &AtomicI64| Some(a.load(Ordering::Relaxed)).filter(|&v| v != NO_VER);
        let conns = self
            .conns
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|c| ConnHealth {
                conn: c.name.clone(),
                alive_ms: opt(c.alive.load(Ordering::Relaxed)),
                started_ms: opt(c.started.load(Ordering::Relaxed)),
                pairs: c
                    .pairs
                    .iter()
                    .map(|p| PairHealth {
                        ticker: p.ticker.clone(),
                        last_ms: opt(p.last.load(Ordering::Relaxed)),
                        since_ms: p.since.load(Ordering::Relaxed),
                        last_id: ver(&p.id),
                        last_changed_ms: ver(&p.changed),
                    })
                    .collect(),
            })
            .collect();
        FeedHealth {
            scheduled,
            per_pair,
            confirm,
            conns,
        }
    }

    /// Команда соединению для методов контракта (resubscribe_pair, reconnect_conn). Принимает: площадку, имя
    /// соединения, команду. Ошибки: BadResponse — соединения нет или оно остановлено (текст причины).
    pub fn command(
        &self,
        venue: crate::core::types::VenueId,
        conn: &str,
        cmd: Cmd,
    ) -> super::errors::GatewayResult<()> {
        self.send(conn, cmd).map_err(|m| {
            super::errors::GatewayError::new(super::errors::ErrorCode::BadResponse, Some(venue), m)
        })
    }

    /// Команда соединению по имени. Ошибка — текст: соединения нет или его задача уже закончилась.
    pub fn send(&self, conn: &str, cmd: Cmd) -> Result<(), String> {
        let list = self.conns.read().unwrap_or_else(|e| e.into_inner());
        let c = list
            .iter()
            .find(|c| c.name == conn)
            .ok_or_else(|| format!("соединения {conn} нет в текущей подписке"))?;
        c.tx.send(cmd)
            .map_err(|_| format!("соединение {conn} уже остановлено"))
    }
}

impl ConnHandle {
    /// Новая сессия (подключились): прогрев соединения и всех пар — с этого момента; прежние кадры и их версии
    /// забыты. Ошибок нет.
    pub fn session_started(&self) {
        let now = now_ms();
        self.conn.started.store(now, Ordering::Relaxed);
        self.conn.alive.store(now, Ordering::Relaxed);
        for p in &self.conn.pairs {
            p.last.store(0, Ordering::Relaxed);
            p.since.store(now, Ordering::Relaxed);
            p.id.store(NO_VER, Ordering::Relaxed);
            p.changed.store(NO_VER, Ordering::Relaxed);
        }
    }

    /// Кадр соединения любого вида. Ошибок нет.
    pub fn alive(&self) {
        self.conn.alive.store(now_ms(), Ordering::Relaxed);
    }

    /// Кадр стакана пары (и кадр соединения), без версии книги. Неизвестный тикер — только кадр соединения.
    /// Ошибок нет.
    pub fn frame(&self, ticker: &str) {
        self.frame_ver(ticker, None, None);
    }

    /// Решение 213: кадр стакана пары с версией книги из кадра — номер (та же шкала, что BookVersion.id по
    /// документации биржи) и/или время изменения по часам биржи (та же шкала, что BookVersion.changed_ms). Отмечать на
    /// КАЖДОМ кадре, в том числе на кадре с тем же номером, который ядру не отдаётся (решение 109). None — биржа такого
    /// поля не даёт (прежнее значение затирается: версия должна быть именно из последнего кадра). Ошибок нет.
    pub fn frame_ver(&self, ticker: &str, id: Option<i64>, changed_ms: Option<i64>) {
        let now = now_ms();
        self.conn.alive.store(now, Ordering::Relaxed);
        if let Some(&i) = self.idx.get(ticker) {
            let p = &self.conn.pairs[i];
            p.last.store(now, Ordering::Relaxed);
            p.id.store(id.unwrap_or(NO_VER), Ordering::Relaxed);
            p.changed
                .store(changed_ms.unwrap_or(NO_VER), Ordering::Relaxed);
        }
    }

    /// Пара переподписана: прогрев пары — с этого момента. Ошибок нет.
    pub fn resubscribed(&self, ticker: &str) {
        if let Some(&i) = self.idx.get(ticker) {
            self.conn.pairs[i].since.store(now_ms(), Ordering::Relaxed);
        }
    }
}
