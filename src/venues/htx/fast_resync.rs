//! Файл: venues/htx/fast_resync.rs
//! Что делает: решение 422 — кому передать просьбу «пересинхронизируй быстрый стакан пары»: тикер → (символ HTX,
//! канал своего соединения быстрого канала). Снимки (snap_out.rs) нашли расхождение — просьба уходит соединению /feed
//! этой пары (fast_conn.rs), оно сбрасывает локальный стакан и запрашивает снимок.
//! Кто вызывает: fast_ws.rs (register), snap_out.rs (send), book_mode.rs (clear при новой подписке).
//! От чего зависит: tokio (mpsc).
//! Настройки: нет.

use std::collections::HashMap;
use std::sync::Mutex;

use tokio::sync::mpsc::UnboundedSender;

/// Тикер → (символ HTX, канал соединения).
#[derive(Default)]
pub struct Resync(Mutex<HashMap<String, (String, UnboundedSender<String>)>>);

impl Resync {
    /// Соединение быстрого канала берёт пары: (тикер, символ) и свой канал. Ошибок нет.
    pub fn register(&self, pairs: &[(String, String)], tx: &UnboundedSender<String>) {
        let mut r = self.0.lock().unwrap_or_else(|p| p.into_inner());
        for (ticker, symbol) in pairs {
            r.insert(ticker.clone(), (symbol.clone(), tx.clone()));
        }
    }

    /// Просит соединение пары пересинхронизировать её. Ошибок нет (соединения нет — ничего: новая сессия начнёт со
    /// снимка).
    pub fn send(&self, ticker: &str) {
        let r = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((symbol, tx)) = r.get(ticker) {
            let _ = tx.send(symbol.clone());
        }
    }

    /// Забывает все соединения (новая подписка). Ошибок нет.
    pub fn clear(&self) {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).clear();
    }
}
