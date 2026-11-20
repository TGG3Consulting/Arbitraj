//! Файл: venues/bybit/backup_kind.rs
//! Что делает: решение 428 — типы выбора источника стакана (backup.rs): стакан страховки в памяти (Held), состояние
//! основного потока пары (Main), решения по кадру основного / страховки и по потере основного, отпечаток первых K
//! уровней для сверки двух глубин (print). Вынесено из backup.rs (файл ≤ 200 строк).
//! Кто вызывает: backup.rs, ws_book.rs, ws_give.rs.
//! От чего зависит: core/types (OrderBook, PriceLevel), book.rs (Alive).
//! Настройки: нет.
//!
//! Сверка по отпечатку: Orderbook (https://bybit-exchange.github.io/docs/v5/websocket/public/orderbook) — seq «You can use
//! this field to compare different levels orderbook data»; уровни — [цена, количество] строками, bids по убыванию
//! цены, asks по возрастанию. Сравниваются уровни, которые есть в обеих глубинах: первые K = меньшая глубина.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::time::Instant;

use crate::core::types::{OrderBook, PriceLevel};

use super::book::Alive;

/// Последний стакан страховки пары: стакан в формате ядра (venue.depth_levels верхних уровней), живость её
/// соединения, seq кадра, когда пришёл кадр.
#[derive(Clone)]
pub struct Held {
    pub book: OrderBook,
    pub alive: Alive,
    pub seq: i64,
    pub at: Instant,
}

/// Состояние основного потока пары.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Main {
    /// Снимка ещё не было (начало сессии) или соединение / подписка потеряны.
    #[default]
    Off,
    /// Разрыв или расхождение: ждём новый снимок (изменения до него ядру не идут).
    Resync,
    /// Синхронизирован: источник ядра.
    Synced,
}

/// Отпечаток первых уровней: сколько уровней взято (K) и хеш цен и количеств обеих сторон.
pub type Print = (usize, u64);

/// Что делать с кадром основного потока.
#[derive(Debug, PartialEq, Eq)]
pub enum MainAct {
    /// Отдан ядру.
    Give,
    /// Отдан ядру, и до этого стакан ядра был из страховки (возврат на основной).
    Back,
    /// Не отдавать (ждём снимок после разрыва или seq не новее отданного).
    Skip,
    /// Расхождение со страховкой при равном seq: основной — пересинхронизация; отдать страховку (если есть).
    Mismatch,
}

/// Что делать при потере основного (разрыв, обрыв, отказ подписки).
pub enum Lost {
    /// Стакан страховки отдан ядру (переключение на страховку).
    Gave,
    /// Стакан ядра уже из страховки — ничего не менять.
    Keep,
    /// Страховки нет, она устарела или старее отданного — стакан пары недостоверен (как без страховки).
    Invalidate,
}

/// Что делать с кадром страховки.
pub enum BackupAct {
    /// Отдать ядру (основной не синхронизирован).
    Give,
    /// Только запомнить.
    Keep,
    /// Расхождение с основным при равном seq: основной пересинхронизировать через его соединение (имя), отдать страховку.
    Mismatch(Option<String>),
}

/// Отпечаток первых k уровней обеих сторон. Принимает: стороны (уже по порядку), k. Возвращает: (k, хеш). Ошибок нет.
pub fn print(bids: &[PriceLevel], asks: &[PriceLevel], k: usize) -> Print {
    let mut h = DefaultHasher::new();
    for side in [bids, asks] {
        let top = &side[..side.len().min(k)];
        top.len().hash(&mut h);
        for l in top {
            l.price.hash(&mut h);
            l.qty.hash(&mut h);
        }
    }
    (k, h.finish())
}
