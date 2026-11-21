//! Файл: gateway/tap.rs
//! Что делает: стык записи окон (решение 125) — сырые кадры площадки по одной монете на время окна. Ядро открывает
//! окно и отдаёт адаптеру «приёмник» (TapSink); адаптер кладёт в него каждый кадр стакана этой монеты (из уже
//! идущего потока рынка) и каждый кадр канала сделок (своё соединение на время окна) — как получил, после снятия
//! сжатия транспорта (BingX — GZIP), со временем получения в наносекундах. Кадры не ждут: приёмник полон — кадр
//! теряется и считается (lost). Список приёмников адаптера — Taps (по тикеру монеты).
//! Кто вызывает: адаптеры бирж (venues/bingx, venues/mexc — Taps, TapFrame), ядро (core/window_tap.rs — TapSink).
//! От чего зависит: core/types (VenueId), tokio.
//! Настройки: нет (ёмкость приёмника — window.queue_max, задаёт ядро при открытии окна).
//!
//! Это стык: менять только через запрос на изменение стыка (docs/AGENTS.md).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, watch};

use crate::core::types::VenueId;

/// Какой поток площадки.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TapStream {
    /// Стакан монеты — канал, на который адаптер уже подписан для торговли.
    Book,
    /// Сделки монеты — публичный канал сделок, подписка на время окна.
    Trades,
    /// Изменения стакана всей глубины — инкрементальный канал, подписка на время окна (дополнение решения 125).
    Diff,
    /// Снимок стакана по REST, с которого начинают изменения (MEXC: /api/v3/depth в начале окна и при разрыве версий).
    Snapshot,
}

/// Содержимое кадра — ровно то, что прислала площадка (после снятия сжатия транспорта).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TapData {
    /// Текстовый кадр (JSON).
    Text(String),
    /// Двоичный кадр (MEXC — protobuf).
    Binary(Vec<u8>),
}

impl TapData {
    /// Размер содержимого, байт. Ошибок нет.
    pub fn len(&self) -> usize {
        match self {
            TapData::Text(t) => t.len(),
            TapData::Binary(b) => b.len(),
        }
    }

    /// Пустой ли кадр. Ошибок нет.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Один кадр окна.
#[derive(Debug, Clone)]
pub struct TapFrame {
    pub venue: VenueId,
    pub stream: TapStream,
    /// Имя соединения адаптера (для разбора записи).
    pub conn: String,
    /// Время получения кадра ботом, Unix, наносекунды.
    pub recv_ns: i64,
    pub data: TapData,
}

/// Приёмник кадров одного окна. Дешёво клонируется.
#[derive(Debug, Clone)]
pub struct TapSink {
    tx: mpsc::Sender<TapFrame>,
    /// Сколько кадров потеряно: приёмник полон (запись не успевает).
    lost: Arc<AtomicU64>,
}

impl TapSink {
    /// Новый приёмник. Принимает: канал (ёмкость задаёт ядро), счётчик потерь. Ошибок нет.
    pub fn new(tx: mpsc::Sender<TapFrame>, lost: Arc<AtomicU64>) -> Self {
        Self { tx, lost }
    }

    /// Кладёт кадр без ожидания. Полон — потеря (счётчик). Закрыт (окно закрыто) — кадр не нужен.
    /// Принимает: кадр. Ошибок нет.
    pub fn push(&self, f: TapFrame) {
        if let Err(mpsc::error::TrySendError::Full(_)) = self.tx.try_send(f) {
            self.lost.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Что адаптер включил для окна — показывается на странице окна.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TapInfo {
    /// Канал стакана (как у площадки), например «CAKE-USDT@depth20».
    pub book_channel: String,
    /// Канал сделок; Err — почему сделки не пишутся (например не задана настройка).
    pub trades_channel: Result<String, String>,
    /// Как площадка присылает сделки (по официальной документации): «по одной сделке в кадре» или «пачками …».
    pub trades_form: String,
    /// Канал изменений стакана всей глубины; Err — почему не пишется.
    #[serde(default = "no_diff")]
    pub diff_channel: Result<String, String>,
}

/// Сводки окон до дополнения решения 125 — канала изменений не было. Ошибок нет.
fn no_diff() -> Result<String, String> {
    Err("не было в этой версии".into())
}

/// Запись в списке приёмников адаптера: приёмник и выключатель соединения сделок.
struct Entry {
    sink: TapSink,
    stop: watch::Sender<bool>,
}

/// Приёмники окон адаптера по тикеру монеты. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Taps {
    map: Arc<RwLock<HashMap<String, Entry>>>,
    /// Сколько приёмников — быстрый ответ «пусто» без блокировки на каждом кадре.
    count: Arc<AtomicUsize>,
}

impl Taps {
    /// Добавляет приёмник монеты (прежний заменяется и его соединение сделок останавливается).
    /// Принимает: тикер, приёмник. Возвращает: выключатель для соединения сделок. Ошибок нет.
    pub fn add(&self, ticker: &str, sink: TapSink) -> watch::Receiver<bool> {
        let (stop, rx) = watch::channel(false);
        let old = self
            .map
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .insert(ticker.to_string(), Entry { sink, stop });
        match old {
            Some(o) => {
                let _ = o.stop.send(true);
            }
            None => {
                self.count.fetch_add(1, Ordering::Relaxed);
            }
        }
        rx
    }

    /// Убирает приёмник монеты и останавливает её соединение сделок. Принимает: тикер.
    /// Возвращает: был ли приёмник. Ошибок нет.
    pub fn remove(&self, ticker: &str) -> bool {
        let old = self
            .map
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .remove(ticker);
        let Some(o) = old else { return false };
        self.count.fetch_sub(1, Ordering::Relaxed);
        let _ = o.stop.send(true);
        true
    }

    /// Пишется ли монета сейчас. Ошибок нет.
    pub fn has(&self, ticker: &str) -> bool {
        self.count.load(Ordering::Relaxed) > 0
            && self
                .map
                .read()
                .unwrap_or_else(|p| p.into_inner())
                .contains_key(ticker)
    }

    /// Кадр монеты — в её приёмник, если монета пишется (иначе ничего, и данные не собираются: `data` вызывается
    /// только при записи). Принимает: площадку, тикер, поток, соединение, время получения нс, содержимое. Ошибок нет.
    pub fn push(
        &self,
        venue: &str,
        ticker: &str,
        stream: TapStream,
        conn: &str,
        recv_ns: i64,
        data: impl FnOnce() -> TapData,
    ) {
        if self.count.load(Ordering::Relaxed) == 0 {
            return;
        }
        let sink = self
            .map
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(ticker)
            .map(|e| e.sink.clone());
        if let Some(s) = sink {
            s.push(TapFrame {
                venue: VenueId(venue.to_string()),
                stream,
                conn: conn.to_string(),
                recv_ns,
                data: data(),
            });
        }
    }
}

/// Время сейчас, Unix, наносекунды. Ошибок нет (вне диапазона i64 — 0).
pub fn now_ns() -> i64 {
    chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
}
