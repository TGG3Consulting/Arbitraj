//! Файл: api/ws.rs
//! Что делает: WebSocket для интерфейса — живой поток изменений настроек, записей журнала и событий ядра.
//! Сообщение: {"type":"settings","data":{…}}, {"type":"journal","data":{…}} или событие ядра
//! (core/view.rs: coin_check, pool_check, coin_block, stuck_alert, manual, send_channel, gas_stats,
//! spread, coins, candidate, deal, balances, control).
//! Кто вызывает: интерфейс (подключается к /api/ws).
//! От чего зависит: api/state.rs, settings/live.rs, storage/journal.rs, core (Core::subscribe).
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
    let mut core = st.core.subscribe();
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
            // События ядра уже в виде {"type": …, "data": …} (core/view.rs).
            r = core.recv() => match r {
                Ok(e) => serde_json::to_value(e).unwrap_or_else(|e| json!({ "type": "error", "data": e.to_string() })),
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
