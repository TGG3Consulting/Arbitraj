//! Файл: tests/core_step_timing.rs
//! Что делает: решение 265 — «Время по шагам»: (1) подбор объёма на записанных стакане BingX и пуле CAKE/USDT V2
//! (tests/size_common) под замером (core/step_timing.rs measure) даёт строку с этапом «подбор объёма», числом расчётов
//! объёма и средним на расчёт, частями «границы (бисекция)», «точки (изломы и сетка)», «расчёт объёма», «вне этапов»;
//! (2) HTTP-запрос общего обмена шлюза (gateway/http.rs exchange_full) под замером попадает в строку ордера по имени
//! «метод путь» с кодом ответа, а обмен без журнала (exchange — узел BSC, агрегатор) — нет. Тело ответа — записанный
//! ответ MEXC exchangeInfo (tests/data/mexc_2026-11-02/exchange_info.json), его отдаёт локальный сервер.
//! Числа времени не проверяются — только названия шагов.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{step_timing, calc_timing, size_search}, arb::gateway::http, tests/size_common.

mod size_common;

use std::time::Duration;

use arb::core::calc_timing::stage;
use arb::core::size_search::search;
use arb::core::step_timing::{HEAD, Steps, measure};
use arb::gateway::http::{Limits, exchange, exchange_full};
use rust_decimal::Decimal;
use size_common::curve::{Dir, RecCurve};
use size_common::{Rec, d, params};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[tokio::test]
async fn size_search_steps_are_named() {
    let rec = Rec::load("step-timing").await;
    let p = params(&rec, Dir::DexToBingx, d("20000"));
    let mut c = RecCurve {
        rec: &rec,
        dir: Dir::DexToBingx,
        slip: Decimal::ONE,
    };
    let (found, m) = measure(stage(|| "подбор объёма".into(), search(&mut c, &p))).await;
    let f = found.unwrap();
    let text = m.calc_text("пересчёт с финальной котировкой");
    println!("{text}");
    for name in [
        HEAD,
        "пересчёт с финальной котировкой, всего",
        "подбор объёма",
        "расчётов объёма",
        "мс на расчёт",
        "границы (бисекция)",
        "точки (изломы и сетка)",
        "расчёт объёма",
        "вне этапов",
        "HTTP-запросов к биржам нет",
        "запросов к узлу BSC и агрегатору нет",
    ] {
        assert!(text.contains(name), "нет «{name}»: {text}");
    }
    // Число расчётов объёма в строке — то же, что посчитал подбор (без повторов из памяти).
    assert!(
        text.contains(&format!("расчётов объёма {}", f.choice.evals)),
        "{text}"
    );
    if f.choice.peaks > 0 {
        assert!(text.contains("уточнение"), "{text}");
    }
}

/// Сервер: на каждое соединение — ответ 200 с записанным телом. Возвращает: адрес.
async fn serve(body: String) -> String {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            let body = body.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 64 * 1024];
                let _ = s.read(&mut buf).await;
                let head = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                let _ = s.write_all(head.as_bytes()).await;
                let _ = s.write_all(body.as_bytes()).await;
            });
        }
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn order_requests_are_listed() {
    let body = std::fs::read_to_string("tests/data/mexc_2026-11-02/exchange_info.json").unwrap();
    let base = serve(body).await;
    let lim = Limits {
        head: Duration::from_secs(5),
        idle: Duration::from_secs(5),
        total: Duration::from_secs(10),
        max_bytes: 64 * 1024 * 1024,
    };
    let http = reqwest::Client::new();
    let url = format!("{base}/api/v3/exchangeInfo?symbol=CAKEUSDT&signature=secret");
    let ((), m) = measure(async {
        exchange_full(http.get(&url), lim).await.unwrap();
        // Обмен без журнала (узел BSC, агрегатор) — не в списке.
        exchange(http.post(format!("{base}/node")), lim)
            .await
            .unwrap();
    })
    .await;
    let text = m.reqs_text("ордер покупки mexc");
    println!("{text}");
    assert_eq!(m.reqs.len(), 1, "{text}");
    for name in [
        HEAD,
        "ордер покупки mexc",
        "GET /api/v3/exchangeInfo",
        "код 200",
        "с +",
    ] {
        assert!(text.contains(name), "нет «{name}»: {text}");
    }
    assert!(
        !text.contains("secret") && !text.contains("/node"),
        "{text}"
    );
    // Без замера журнала нет — обмен идёт как раньше.
    exchange_full(http.get(&url), lim).await.unwrap();
    let mut s = Steps::default();
    s.mark("ордер покупки mexc", std::time::Instant::now());
    assert!(s.text("исполнение до покупки").starts_with(HEAD));
}
