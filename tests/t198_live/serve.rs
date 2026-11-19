//! Файл: tests/t198_live/serve.rs
//! Что делает: локальный HTTP-сервер стенда t198_live (ответы — шапка tests/t198_live/mod.rs).
//! Кто вызывает: tests/t198_live/mod.rs (start).
//! От чего зависит: tokio, serde_json; Stand.

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::{Hit, Stand};

/// Тело ответа XT {"rc": 0, "mc": "SUCCESS", "ma": [], "result": …}.
fn ok(result: Value) -> String {
    json!({"rc": 0, "mc": "SUCCESS", "ma": [], "result": result}).to_string()
}

/// Запрос — в журнал стенда (вид, время прихода). Ошибок нет.
fn hit(s: &Stand, h: Hit) {
    s.hits.lock().unwrap().push((h, std::time::Instant::now()));
}

/// Ответ сервера (шапка). Принимает: метод, путь, ячейку. Возвращает: (статус, тело, задержка мс).
fn answer(method: &str, path: &str, s: &Stand) -> (&'static str, String, u64) {
    if path.starts_with("/v4/public/depth") {
        hit(s, Hit::Depth);
        let t =
            std::fs::read_to_string("tests/data/rest_book_2026-11-09/xt_depth_doc.json").unwrap();
        return ("200 OK", t, 0);
    }
    if path.starts_with("/v4/public/symbol") {
        let t = std::fs::read_to_string("tests/data/xt_live_2026-11-06/symbol_pepe.json").unwrap();
        return ("200 OK", ok(serde_json::from_str(&t).unwrap()), 0);
    }
    if path.starts_with("/v4/balances") {
        let btc = s.btc.lock().unwrap().clone();
        let a =
            json!({"assets": [{"currency": "btc", "availableAmount": btc, "frozenAmount": "0"}]});
        return ("200 OK", ok(a), *s.delay_ms.lock().unwrap());
    }
    // GET /v4/balance?currency= (https://doc.xt.com/docs/spot/Balance/GetSingleCurrencyAsset: одна монета, поля как
    // у assets[] Get balances) — «Продать сейчас» берёт количество по реальному балансу (manual_halted::on_hand).
    if path.starts_with("/v4/balance?") {
        let btc = s.btc.lock().unwrap().clone();
        let a = json!({"currency": "btc", "frozenAmount": "0", "availableAmount": btc, "totalAmount": btc});
        return ("200 OK", ok(a), *s.delay_ms.lock().unwrap());
    }
    if path.starts_with("/v4/order") && method == "POST" {
        hit(s, Hit::Post);
        return (
            "200 OK",
            ok(json!({"orderId": "L1"})),
            *s.post_ms.lock().unwrap(),
        );
    }
    if path.starts_with("/v4/order") && method == "GET" {
        hit(s, Hit::Get);
        let o = json!({"symbol": "btc_usdt", "orderId": "L1", "state": "CANCELED", "type": "LIMIT",
                       "side": "SELL", "origQty": "1", "executedQty": "0", "leavingQty": "1",
                       "avgPrice": "0", "tradeQuote": "0", "fee": "0", "feeCurrency": "usdt"});
        return ("200 OK", ok(o), 0);
    }
    ("404 Not Found", "{}".to_string(), 0)
}

/// Запрос целиком (заголовки и тело по content-length, как tests/t175_common). Возвращает: (метод, путь).
async fn request(c: &mut tokio::net::TcpStream) -> (String, String) {
    let mut buf = Vec::new();
    let mut chunk = vec![0u8; 16 * 1024];
    loop {
        let n = c.read(&mut chunk).await.unwrap_or(0);
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        let text = String::from_utf8_lossy(&buf).to_string();
        let Some(cut) = text.find("\r\n\r\n") else {
            continue;
        };
        let need = text[..cut]
            .lines()
            .find_map(|l| {
                let (k, v) = l.split_once(':')?;
                k.eq_ignore_ascii_case("content-length")
                    .then(|| v.trim().parse::<usize>().ok())?
            })
            .unwrap_or(0);
        if buf.len() >= cut + 4 + need {
            break;
        }
    }
    let text = String::from_utf8_lossy(&buf).to_string();
    let mut w = text.split_whitespace();
    let m = w.next().unwrap_or("").to_string();
    (m, w.next().unwrap_or("").to_string())
}

/// Локальный сервер. Возвращает: адрес.
pub async fn serve(s: Stand) -> String {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move {
        loop {
            let Ok((mut c, _)) = l.accept().await else {
                return;
            };
            let s = s.clone();
            tokio::spawn(async move {
                let (method, path) = request(&mut c).await;
                let (code, body, wait) = answer(&method, &path, &s);
                if wait > 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(wait)).await;
                }
                let r = format!(
                    "HTTP/1.1 {code}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = c.write_all(r.as_bytes()).await;
            });
        }
    });
    addr
}
