//! Файл: api/ws.rs
//! Что делает: WebSocket для интерфейса — живой поток изменений настроек, записей журнала и событий ядра.
//! Сообщение: {"type":"settings","data":{…}}, {"type":"journal","data":{…}} или событие ядра
//! (core/view.rs: coin_check, pool_check, coin_block, stuck_alert, manual, send_channel, gas_stats,
//! spread, coins, candidate, deal, balances, control).
//! Отставание: {"type":"lagged","skipped":N,"source":"settings"|"journal"|"core"} — интерфейс перечитывает по REST
//! только то, что зависит от этого источника (решение 109: раньше на любое отставание перечитывалось всё, и при
//! потоке строк «Живого экрана» интерфейс уходил в бесконечное перечитывание). Отставания одного источника
//! склеиваются: не чаще одного сообщения в live.lagged_min_interval_ms (остальные — одним сообщением позже, skipped —
//! сумма). Не задано — каждое сразу, как раньше.
//! Кто вызывает: интерфейс (подключается к /api/ws).
//! От чего зависит: api/state.rs, settings/live.rs, storage/journal.rs, core (Core::subscribe).
//! Настройки: live.lagged_min_interval_ms.

use std::time::Duration;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use serde_json::{Value, json};
use tokio::sync::broadcast::error::RecvError;
use tokio::time::Instant;

use super::state::AppState;

/// Источники живого потока — в порядке индексов `Lag`.
const SOURCES: [&str; 3] = ["settings", "journal", "core"];

/// Склейка отставаний по источникам: когда последний раз сообщили и сколько пропущено с тех пор.
#[derive(Default)]
struct Lag {
    last: [Option<Instant>; 3],
    pending: [u64; 3],
}

impl Lag {
    /// Отставание источника i на n событий. Возвращает сообщение, если его пора отправить; иначе копит.
    fn on(&mut self, i: usize, n: u64, min: Option<Duration>) -> Option<Value> {
        self.pending[i] += n;
        let due = match (self.last[i], min) {
            (Some(at), Some(min)) => at.elapsed() >= min,
            _ => true,
        };
        due.then(|| self.flush(i))
    }

    /// Сообщение об отставании источника i с накопленным числом; сбрасывает счётчик.
    fn flush(&mut self, i: usize) -> Value {
        let n = std::mem::take(&mut self.pending[i]);
        self.last[i] = Some(Instant::now());
        json!({ "type": "lagged", "skipped": n, "source": SOURCES[i] })
    }

    /// Когда отправить накопленное (самое раннее). None — копить нечего.
    fn next_due(&self, min: Option<Duration>) -> Option<Instant> {
        let min = min?;
        (0..3)
            .filter(|&i| self.pending[i] > 0)
            .filter_map(|i| self.last[i].map(|at| at + min))
            .min()
    }
}

/// Принимает подключение WebSocket.
pub async fn upgrade(State(st): State<AppState>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| stream(socket, st))
}

/// Шлёт события в сокет, пока интерфейс подключён. Отставание — см. шапку файла.
async fn stream(mut socket: WebSocket, st: AppState) {
    let mut changes = st.changes.subscribe();
    let mut journal = st.journal_live.subscribe();
    let mut core = st.core.subscribe();
    let mut lag = Lag::default();
    loop {
        let min = st
            .settings
            .integer("live.lagged_min_interval_ms", Default::default())
            .ok()
            .filter(|ms| *ms > 0)
            .map(Duration::from_millis);
        let due = lag.next_due(min);
        let msg = tokio::select! {
            r = changes.recv() => match r {
                Ok(c) => Some(json!({ "type": "settings", "data": c })),
                Err(RecvError::Lagged(n)) => lag.on(0, n, min),
                Err(RecvError::Closed) => break,
            },
            r = journal.recv() => match r {
                Ok(e) => Some(json!({ "type": "journal", "data": e })),
                Err(RecvError::Lagged(n)) => lag.on(1, n, min),
                Err(RecvError::Closed) => break,
            },
            // События ядра уже в виде {"type": …, "data": …} (core/view.rs).
            r = core.recv() => match r {
                Ok(e) => Some(serde_json::to_value(e).unwrap_or_else(|e| json!({ "type": "error", "data": e.to_string() }))),
                Err(RecvError::Lagged(n)) => lag.on(2, n, min),
                Err(RecvError::Closed) => break,
            },
            // Пора отправить накопленные отставания.
            _ = async { tokio::time::sleep_until(due.unwrap_or_else(Instant::now)).await }, if due.is_some() => {
                let ready: Vec<usize> = (0..3).filter(|&i| lag.pending[i] > 0).collect();
                for i in ready {
                    let m = lag.flush(i);
                    if socket.send(Message::Text(m.to_string().into())).await.is_err() {
                        return;
                    }
                }
                None
            },
            incoming = socket.recv() => match incoming {
                // Интерфейс закрыл соединение или оно оборвалось.
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break,
                Some(Ok(_)) => continue,
            },
        };
        let Some(msg) = msg else { continue };
        if socket
            .send(Message::Text(msg.to_string().into()))
            .await
            .is_err()
        {
            break;
        }
    }
}
