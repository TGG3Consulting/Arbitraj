//! Файл: tests/core_sell_turn_live_t200.rs
//! Что делает: решение 415 (ревью t200, С-1) — очередь продажи счёта через НАСТОЯЩИЙ путь продажи: две сделки BTC на
//! xt одновременно идут в sell::sell_or_stuck на стенде tests/t198_live (живое ядро на SQLite, настоящий адаптер XT в
//! локальный сервер по формату официальной документации XT: справочник пар — живая запись, стакан — пример
//! документации, баланс BTC — ячейка стенда, ордер IOC снят без исполнения). Ответ POST /v4/order задержан на 300 мс
//! (окно «ордер первой в полёте»); стенд пишет приход каждого запроса стакана и ордера.
//! Проверяется (по журналу стенда):
//! - котировка второй (снимок стакана REST, Quote.at — время его получения) пришла после итога ордера первой (GET
//!   состояния ордера) — до итога первой ровно один запрос стакана (её собственная котировка);
//! - ордера не пересекаются: между постановкой первой и её итогом нет ни запроса стакана, ни второй постановки.
//!
//! Без хода очереди в sell.rs (sell_turn::take) тест падает: факт счёта сделок читается по очереди (credit_lock::fact,
//! исключительный; вторая встаёт в очередь замка раньше, чем первая просит общий доступ на ордер), а дальше вторая
//! котирует и ставит ордер под общим доступом, пока POST первой ждёт 300 мс, — её запрос стакана или её POST
//! приходят в окне ордера первой; снимок REST первой забывается только после ордера (exec_book::forget), поэтому
//! без запроса стакана вторая ставит ордер по старому снимку — её POST тоже в окне.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core (sell, ledger, deal), tests/t198_live.

mod t198_live;

use std::time::{Duration, Instant};

use arb::core::deal::{Deal, Step};
use arb::core::types::{Asset, Direction, VenueId};
use rust_decimal::Decimal;
use serde_json::json;
use t198_live::{Hit, big, start};

/// Сделка BTC на xt (куплено на bsc_dex): 1 BTC, продажа.
fn deal(id: &str) -> Deal {
    let dir = Direction {
        asset: Asset {
            ticker: "BTC".into(),
            bsc_contract: None,
        },
        buy_on: VenueId("bsc_dex".into()),
        sell_on: VenueId("xt".into()),
    };
    let mut x = Deal::new(id.into(), dir, json!(null), Decimal::ZERO);
    x.at = VenueId("xt".into());
    x.qty = Decimal::ONE;
    x.step = Step::Selling;
    x
}

/// Папка теста (новая).
fn dir(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("arb-t200-live-{name}-{}", std::process::id()))
}

#[test]
fn second_sale_quotes_after_first_order() {
    big(async {
        // Баланс 2 BTC: у каждой сделки по факту её 1 BTC (вторая — на счёте 2, другой 1).
        let (core, stand) = start(&dir("two"), "2").await;
        *stand.post_ms.lock().unwrap() = 300;
        let (a, b) = (deal("T200X1"), deal("T200X2"));
        arb::core::ledger::save(&core, &a).await;
        arb::core::ledger::save(&core, &b).await;
        let (c1, c2) = (core.clone(), core.clone());
        let h1 = tokio::spawn(async move { arb::core::sell::sell_or_stuck(&c1, a).await });
        let h2 = tokio::spawn(async move { arb::core::sell::sell_or_stuck(&c2, b).await });
        for h in [h1, h2] {
            tokio::time::timeout(Duration::from_secs(30), h)
                .await
                .expect("продажа закончилась")
                .unwrap();
        }
        let hits: Vec<(Hit, Instant)> = stand.hits.lock().unwrap().clone();
        let at = |k: Hit| -> Vec<Instant> {
            hits.iter()
                .filter(|(h, _)| *h == k)
                .map(|(_, t)| *t)
                .collect()
        };
        let posts = at(Hit::Post);
        assert_eq!(posts.len(), 2, "обе продажи дошли до ордера: {hits:?}");
        let p1 = posts[0];
        let g1 = at(Hit::Get)
            .into_iter()
            .find(|t| *t > p1)
            .expect("итог ордера первой");
        // Ордера не пересекаются и котировки нет, пока ордер первой в полёте.
        assert!(
            !hits
                .iter()
                .any(|(h, t)| *h != Hit::Get && *t > p1 && *t < g1),
            "в окне ордера первой — ни котировки, ни второй постановки: {hits:?}"
        );
        assert!(posts[1] > g1, "вторая постановка — после итога первой");
        // До итога первой — только её котировка; котировка второй — после итога первой и до её постановки.
        let depth = at(Hit::Depth);
        assert_eq!(
            depth.iter().filter(|t| **t < g1).count(),
            1,
            "до итога первой — одна котировка: {hits:?}"
        );
        assert!(
            depth.iter().any(|t| *t > g1 && *t < posts[1]),
            "котировка второй — по стакану после итога первой: {hits:?}"
        );
        // Обе продажи прошли настоящий путь: строка котировки и худшей цены в хронологии каждой.
        for id in ["T200X1", "T200X2"] {
            let x = core.deals().into_iter().find(|d| d.id == id).unwrap();
            assert!(
                x.timeline
                    .iter()
                    .any(|(_, t)| t.starts_with("Котировка продажи на xt")),
                "{id}: {:?}",
                x.timeline
            );
        }
    });
}
