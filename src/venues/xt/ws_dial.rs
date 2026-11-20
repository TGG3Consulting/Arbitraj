//! Файл: venues/xt/ws_dial.rs
//! Что делает: открывает соединение публичного WebSocket XT.com со сжатием permessage-deflate (RFC 7692):
//! с 2026-12-10 08:00 UTC XT отвергает публичные соединения без сжатия (решение 419).
//! Кто вызывает: ws_conn.rs (session).
//! От чего зависит: yawc (клиент WebSocket с permessage-deflate), url.
//! Настройки: своих нет; адрес и время на подключение — из ws.rs (Cfg: venue.ws_url, http.timeout_ms).
//!
//! Официально (https://doc.xt.com/docs/spot/WebSocket%20Public/wss-general): «Starting December 10, 2026, 08:00 (UTC)
//! (16:00 UTC+8), the public WebSocket market data endpoint will only accept connections that negotiate
//! permessage-deflate compression (RFC 7692). Connections without compression will be rejected.»; «Private streams and
//! REST APIs are not affected.»; «Message content and format are unchanged.»; «the handshake response should include
//! `Sec-WebSocket-Extensions: permessage-deflate`». Параметров расширения (client_max_window_bits,
//! server_no_context_takeover и т.п.) документация не задаёт — клиент предлагает «permessage-deflate» без параметров
//! (как в заголовке документации), параметры ответа сервера yawc применяет сам (RFC 7692, 7.1).
//! Сжатие для XT обязательно — оно включено всегда, без настройки (решение 419).
//! Почему yawc: tokio-tungstenite / tungstenite (0.30 — последняя версия) permessage-deflate не поддерживают и рвут
//! соединение на кадре с битом RSV1 (tungstenite protocol/mod.rs: NonZeroReservedBits). Остальные биржи — на
//! tokio-tungstenite, как прежде.

use std::time::Duration;

use yawc::{Options, TcpWebSocket, WebSocket};

/// Подключается к публичному WebSocket XT с предложением permessage-deflate.
/// Принимает: адрес (venue.ws_url), время на подключение (http.timeout_ms).
/// Возвращает: соединение. Ошибки (текст причины): адрес не разобран, время вышло, сбой TCP / TLS / рукопожатия
/// (в том числе отказ сервера).
pub async fn dial(url: &str, timeout: Duration) -> Result<TcpWebSocket, String> {
    let url = url::Url::parse(url).map_err(|e| format!("адрес venue.ws_url не разобран: {e}"))?;
    // Предложение «permessage-deflate» без параметров; уровень сжатия касается только наших кадров (подписки, ping).
    // Проверка UTF-8 текстовых кадров — как у tokio-tungstenite: нарушение рвёт соединение.
    let opts = Options::default()
        .with_low_latency_compression()
        .with_utf8();
    match tokio::time::timeout(timeout, WebSocket::connect(url).with_options(opts)).await {
        Err(_) => Err("не удалось подключиться за отведённое время".into()),
        Ok(Err(e)) => Err(format!("подключение: {e}")),
        Ok(Ok(ws)) => Ok(ws),
    }
}
