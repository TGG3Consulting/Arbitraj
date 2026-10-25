//! Файл: gateway/http.rs
//! Что делает: один HTTP-обмен с площадкой с двумя раздельными сроками (решение 61, живой запуск 2026-10-25):
//! - http.timeout_ms — до заголовков ответа (соединение, отправка, ожидание ответа сервера);
//! - http.body_idle_ms — чтение тела: сколько ждать очередной порции данных. Пока данные идут, большой ответ (пачка
//!   Multicall3 на 200 вызовов — около 1 МБ, справочник BingX) дочитывается до конца, как бы медленно ни шла сеть;
//!   оборвался или замолчал — ошибка с правдой: «ответ не дочитан», сколько байт получено, а не «нет связи».
//!
//! Общий срок reqwest (RequestBuilder::timeout) не используется: он действует «от начала соединения до конца тела» —
//! https://docs.rs/reqwest/0.13.5/reqwest/struct.RequestBuilder.html#method.timeout — и на медленной сети обрывает
//! большой ответ на середине. Тело читается порциями — Response::chunk:
//! https://docs.rs/reqwest/0.13.5/reqwest/struct.Response.html#method.chunk .
//! Кто вызывает: venues/bsc_dex/node/rpc.rs (узел BSC), venues/bsc_dex/directory/gt_http.rs (индексатор),
//! venues/bingx/rest.rs (REST BingX).
//! От чего зависит: reqwest, tokio.
//! Настройки: нет своих (сроки передаёт вызывающий из http.timeout_ms и http.body_idle_ms).

use std::time::Duration;

use reqwest::RequestBuilder;

/// Сроки обмена.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// До заголовков ответа (http.timeout_ms).
    pub head: Duration,
    /// Тишина при чтении тела (http.body_idle_ms).
    pub idle: Duration,
}

/// Почему обмен не удался. Текст — по-русски, адрес из ошибки reqwest удалён (в нём может быть ключ).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fail {
    /// Соединение не установлено (запрос до сервера не дошёл).
    Connect(String),
    /// Заголовков ответа нет за http.timeout_ms (запрос мог дойти).
    NoHead,
    /// Отправка или ответ сломались до заголовков (не соединение и не срок).
    Send(String),
    /// Тело замолчало дольше http.body_idle_ms; получено байт.
    Stalled { got: usize },
    /// Тело оборвалось при чтении; получено байт, причина.
    Broken { got: usize, why: String },
    /// Тело не в UTF-8.
    NotText,
}

impl Fail {
    /// Понятная причина. Принимает: сроки (для текста). Ошибок нет.
    pub fn text(&self, lim: &Limits) -> String {
        match self {
            Fail::Connect(e) => format!("нет связи (соединение не установлено): {e}"),
            Fail::NoHead => format!(
                "нет ответа за http.timeout_ms ({} мс)",
                lim.head.as_millis()
            ),
            Fail::Send(e) => format!("запрос не прошёл: {e}"),
            Fail::Stalled { got } => format!(
                "ответ не дочитан: данные не приходили дольше http.body_idle_ms ({} мс), получено {got} байт",
                lim.idle.as_millis()
            ),
            Fail::Broken { got, why } => {
                format!("ответ не дочитан: обрыв при чтении тела, получено {got} байт: {why}")
            }
            Fail::NotText => "ответ не в кодировке UTF-8".into(),
        }
    }

    /// Запрос точно не дошёл до сервера (можно повторить на резервном адресе). Ошибок нет.
    pub fn not_sent(&self) -> bool {
        matches!(self, Fail::Connect(_))
    }

    /// Это срок (а не обрыв). Ошибок нет.
    pub fn is_timeout(&self) -> bool {
        matches!(self, Fail::NoHead | Fail::Stalled { .. })
    }
}

/// Отправляет запрос и дочитывает ответ. Принимает: запрос (без общего срока), сроки.
/// Возвращает: (HTTP-код, текст тела). Ошибки: Fail.
pub async fn exchange(req: RequestBuilder, lim: Limits) -> Result<(u16, String), Fail> {
    let mut resp = match tokio::time::timeout(lim.head, req.send()).await {
        Err(_) => return Err(Fail::NoHead),
        Ok(Err(e)) if e.is_connect() => return Err(Fail::Connect(e.without_url().to_string())),
        Ok(Err(e)) if e.is_timeout() => return Err(Fail::NoHead),
        Ok(Err(e)) => return Err(Fail::Send(e.without_url().to_string())),
        Ok(Ok(r)) => r,
    };
    let status = resp.status().as_u16();
    let mut body = Vec::new();
    loop {
        match tokio::time::timeout(lim.idle, resp.chunk()).await {
            Err(_) => return Err(Fail::Stalled { got: body.len() }),
            Ok(Err(e)) => {
                let why = e.without_url().to_string();
                return Err(Fail::Broken {
                    got: body.len(),
                    why,
                });
            }
            Ok(Ok(Some(part))) => body.extend_from_slice(&part),
            Ok(Ok(None)) => break,
        }
    }
    String::from_utf8(body)
        .map(|t| (status, t))
        .map_err(|_| Fail::NotText)
}
