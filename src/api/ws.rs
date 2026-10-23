//! Файл: api/ws.rs
//! Что делает: WebSocket для интерфейса — живой поток изменений настроек и записей журнала.
//! Сообщение: {"type":"settings","data":{…}} или {"type":"journal","data":{…}}.
//! Кто вызывает: интерфейс (подключается к /api/ws).
//! От чего зависит: api/state.rs, settings/live.rs, storage/journal.rs.
//! Настройки: нет.

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;

use super::state::AppState;

/// Принимает подключение WebSocket.
pub async fn upgrade(State(st): State<AppState>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| stream(socket, st))
}

/// Шлёт события в сокет, пока интерфейс подключён.
/// Если интерфейс отстал, шлёт {"type":"lagged","skipped":N} — интерфейс перечитывает данные по REST.
async fn stream(mut socket: WebSocket, st: AppState) {
    let mut changes = st.changes.subscribe();
    let mut journal = st.journal_live.subscribe();
    loop {
        let msg = tokio::select! {
            r = changes.recv() => match r {
                Ok(c) => json!({ "type": "settings", "data": c }),
                Err(RecvError::Lagged(n)) => json!({ "type": "lagged", "skipped": n }),
                Err(RecvError::Closed) => break,
            },
            r = journal.recv() => match r {
                Ok(e) => json!({ "type": "journal", "data": e }),
                Err(RecvError::Lagged(n)) => json!({ "type": "lagged", "skipped": n }),
                Err(RecvError::Closed) => break,
            },
            incoming = socket.recv() => match incoming {
                // Интерфейс закрыл соединение или оно оборвалось.
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break,
                Some(Ok(_)) => continue,
            },
        };
        if socket
            .send(Message::Text(msg.to_string().into()))
            .await
            .is_err()
        {
            break;
        }
    }
}
