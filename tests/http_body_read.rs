//! Файл: tests/http_body_read.rs
//! Что делает: HTTP-обмен с двумя сроками (gateway/http.rs, решение 61) на ЗАПИСАННОМ большом ответе: тело — живой
//! ответ узла BSC на пачку Multicall3.aggregate3 из 115 вызовов (ступень 2 записи 2026-10-25, блок 0x7b936ae, около
//! 1 МБ — tests/data/indexers_2026-10-24/arb_node_responses_3_2026-10-25.json). Тело отдаёт локальный сервер порциями с
//! паузами — как медленная сеть у Тиграна 2026-10-25 (запрос шёл 2,2 с вместо 0,3 с):
//! - общий срок reqwest на весь обмен обрывает такой ответ на середине («error decoding response body» в журнале
//!   было записано как «нет связи»);
//! - с двумя сроками ответ дочитывается целиком и совпадает с записью байт в байт;
//! - замолчавшее тело — «ответ не дочитан … получено N байт», а не «нет связи»; закрытый порт — «нет связи»;
//! - тело по порциям чаще http.body_idle_ms, но дольше http.total_ms — остановка; больше http.max_body_bytes — остановка.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::gateway::http, reqwest, tokio (локальный TCP-сервер отдаёт записанные байты; данные не
//! подменяются). Сроки теста — параметры теста, не настройки системы.

use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use arb::gateway::http::{Fail, Limits, exchange};

/// Записанный ответ узла (тело JSON-RPC) на пачку ступени 2.
fn recorded_body() -> String {
    let text = std::fs::read_to_string(
        "tests/data/indexers_2026-10-24/arb_node_responses_3_2026-10-25.json",
    )
    .unwrap();
    let doc: Value = serde_json::from_str(&text).unwrap();
    let r = doc["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| {
            r["name"]
                .as_str()
                .unwrap()
                .starts_with("s2 Multicall3 aggregate3")
        })
        .unwrap();
    serde_json::to_string(&r["response"]).unwrap()
}

/// Сервер: один ответ 200 с телом `body` порциями по `part` байт, пауза `gap` между порциями; `stop_after` — после
/// стольких байт замолчать (не закрывая соединение). Возвращает: адрес.
async fn serve(body: String, part: usize, gap: Duration, stop_after: Option<usize>) -> String {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut s, _) = l.accept().await.unwrap();
        let mut buf = vec![0u8; 64 * 1024];
        let _ = s.read(&mut buf).await;
        let head = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        );
        s.write_all(head.as_bytes()).await.unwrap();
        let mut sent = 0;
        for chunk in body.as_bytes().chunks(part) {
            if stop_after.is_some_and(|n| sent >= n) {
                tokio::time::sleep(Duration::from_secs(30)).await;
                return;
            }
            if s.write_all(chunk).await.is_err() {
                return;
            }
            sent += chunk.len();
            tokio::time::sleep(gap).await;
        }
    });
    format!("http://{addr}/")
}

const PART: usize = 64 * 1024;
const GAP: Duration = Duration::from_millis(40);

#[tokio::test]
async fn slow_big_body_is_read_whole_with_two_deadlines() {
    let body = recorded_body();
    assert!(
        body.len() > 900_000,
        "в записи ответ около 1 МБ: {}",
        body.len()
    );
    // Общий срок на весь обмен (как было: .timeout(http.timeout_ms)) обрывает медленное тело.
    let url = serve(body.clone(), PART, GAP, None).await;
    let http = reqwest::Client::new();
    let old = http
        .post(&url)
        .timeout(Duration::from_millis(300))
        .send()
        .await;
    let cut = match old {
        Ok(r) => r.text().await.is_err(),
        Err(_) => true,
    };
    assert!(cut, "общий срок обрывает тело, которое идёт дольше срока");
    // Два срока: до заголовков 300 мс, тишина тела 300 мс — тело (~16 порций по 40 мс) дочитано целиком.
    let url = serve(body.clone(), PART, GAP, None).await;
    let lim = Limits {
        head: Duration::from_millis(300),
        idle: Duration::from_millis(300),
        total: Duration::from_secs(10),
        max_bytes: 4_000_000,
    };
    let (status, text) = exchange(http.post(&url), lim).await.unwrap();
    assert_eq!(status.as_u16(), 200);
    assert_eq!(text, body);
}

#[tokio::test]
async fn stalled_body_says_not_read_whole_not_no_link() {
    let body = recorded_body();
    let url = serve(body, PART, GAP, Some(3 * PART)).await;
    let lim = Limits {
        head: Duration::from_millis(300),
        idle: Duration::from_millis(200),
        total: Duration::from_secs(10),
        max_bytes: 4_000_000,
    };
    let f = exchange(reqwest::Client::new().post(&url), lim)
        .await
        .unwrap_err();
    assert_eq!(f, Fail::Stalled { got: 3 * PART });
    let text = f.text(&lim);
    assert!(text.starts_with("ответ не дочитан"), "{text}");
    assert!(
        text.contains("http.body_idle_ms") && text.contains(&(3 * PART).to_string()),
        "{text}"
    );
    assert!(!text.contains("нет связи"));
}

#[tokio::test]
async fn closed_port_is_no_link() {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    drop(l);
    let lim = Limits {
        head: Duration::from_millis(500),
        idle: Duration::from_millis(500),
        total: Duration::from_secs(10),
        max_bytes: 4_000_000,
    };
    let f = exchange(reqwest::Client::new().post(format!("http://{addr}/")), lim)
        .await
        .unwrap_err();
    assert!(f.not_sent(), "{f:?}");
    assert!(f.text(&lim).starts_with("нет связи"));
}

#[tokio::test]
async fn trickling_body_stops_at_total_and_big_body_at_size_limit() {
    let body = recorded_body();
    // Порции идут чаще http.body_idle_ms, но весь обмен дольше http.total_ms — остановка с правдивой причиной.
    let url = serve(body.clone(), PART, GAP, None).await;
    let ms = Duration::from_millis;
    let lim = Limits {
        head: ms(300),
        idle: ms(300),
        total: ms(200),
        max_bytes: 4_000_000,
    };
    let f = exchange(reqwest::Client::new().post(&url), lim)
        .await
        .unwrap_err();
    assert!(
        matches!(f, Fail::Total { got } if got > 0 && got < body.len()),
        "{f:?}"
    );
    assert!(f.text(&lim).contains("http.total_ms"));
    // Тело больше http.max_body_bytes — чтение остановлено, память не растёт.
    let url = serve(body.clone(), PART, GAP, None).await;
    let lim = Limits {
        head: ms(300),
        idle: ms(300),
        total: ms(10_000),
        max_bytes: 3 * PART,
    };
    let f = exchange(reqwest::Client::new().post(&url), lim)
        .await
        .unwrap_err();
    assert!(
        matches!(f, Fail::TooBig { got } if got > 3 * PART && got <= 4 * PART),
        "{f:?}"
    );
    assert!(f.text(&lim).contains("http.max_body_bytes"));
}
