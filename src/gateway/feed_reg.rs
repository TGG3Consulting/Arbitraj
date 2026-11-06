//! Файл: gateway/feed_reg.rs
//! Что делает: решение 202 — общий для адаптеров учёт свежести потока стаканов (FeedReg): соединение регистрирует свои
//! пары, на каждом кадре отмечает время, получает команды сторожа ядра из своего канала; снимок свежести и доставка
//! команд для методов контракта feed_health / resubscribe_pair / reconnect_conn.
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

/// Учёт одного соединения: время кадров, пары, канал команд. 0 в атомике — «не было».
struct Conn {
    name: String,
    alive: AtomicI64,
    started: AtomicI64,
    /// Тикер → (последний кадр, (пере)подписка).
    pairs: Vec<(String, AtomicI64, AtomicI64)>,
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

    /// Регистрирует соединение и его пары (тикеры ядра). Возвращает: ручку и канал команд соединения.
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
                .map(|t| (t.clone(), AtomicI64::new(0), AtomicI64::new(0)))
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

    /// Снимок свежести. Принимает: признаки площадки (по расписанию, переподписка пары). Ошибок нет.
    pub fn health(&self, scheduled: bool, per_pair: bool) -> FeedHealth {
        let opt = |v: i64| (v > 0).then_some(v);
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
                    .map(|(t, last, since)| PairHealth {
                        ticker: t.clone(),
                        last_ms: opt(last.load(Ordering::Relaxed)),
                        since_ms: since.load(Ordering::Relaxed),
                    })
                    .collect(),
            })
            .collect();
        FeedHealth {
            scheduled,
            per_pair,
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
    /// Новая сессия (подключились): прогрев соединения и всех пар — с этого момента; прежние кадры забыты. Ошибок нет.
    pub fn session_started(&self) {
        let now = now_ms();
        self.conn.started.store(now, Ordering::Relaxed);
        self.conn.alive.store(now, Ordering::Relaxed);
        for (_, last, since) in &self.conn.pairs {
            last.store(0, Ordering::Relaxed);
            since.store(now, Ordering::Relaxed);
        }
    }

    /// Кадр соединения любого вида. Ошибок нет.
    pub fn alive(&self) {
        self.conn.alive.store(now_ms(), Ordering::Relaxed);
    }

    /// Кадр стакана пары (и кадр соединения). Неизвестный тикер — только кадр соединения. Ошибок нет.
    pub fn frame(&self, ticker: &str) {
        let now = now_ms();
        self.conn.alive.store(now, Ordering::Relaxed);
        if let Some(&i) = self.idx.get(ticker) {
            self.conn.pairs[i].1.store(now, Ordering::Relaxed);
        }
    }

    /// Пара переподписана: прогрев пары — с этого момента. Ошибок нет.
    pub fn resubscribed(&self, ticker: &str) {
        if let Some(&i) = self.idx.get(ticker) {
            self.conn.pairs[i].2.store(now_ms(), Ordering::Relaxed);
        }
    }
}
