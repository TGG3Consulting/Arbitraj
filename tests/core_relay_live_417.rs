//! Файл: tests/core_relay_live_417.rs
//! Что делает: решение 417 (ревью t201, С-2) — «Перегон» через НАСТОЯЩИЙ путь на стенде tests/t198_live (живое ядро на
//! SQLite, настоящий адаптер XT в локальный сервер по формату официальной документации XT: справочник пар — живая
//! запись, стакан — пример документации tests/data/rest_book_2026-11-09/xt_depth_doc.json (бид 200 × 0.996, 100 ×
//! 0.001, 20 × 10), баланс BTC — ячейка стенда, ордер IOC снят без исполнения). Сделка 1 BTC на xt; вложено — параметр
//! теста (итог при продаже здесь ≥ 0 или < 0). Журнал стенда — запросы стакана, ордеров и вывода.
//!
//! - (а) режим relay, итог здесь ≥ 0 — продажа здесь: ордер на xt есть, запросов вывода нет; stuck.relay_max_loss_pct не
//!   задана — алерта «не задана настройка» нет (правило Тиграна на всех путях продажи здесь, М-2);
//! - (б) итог < 0 — перегона нет (целей на стенде нет: одна площадка), продажа здесь по stuck.relay_max_loss_pct: убыток
//!   сверх порога — алерт человеку, ордера и вывода нет. Ветка «у цели нет ряда σ» требует второй площадки с
//!   котировкой — на стенде её нет; она проверена чистыми функциями (tests/core_relay_reserve_417.rs, params_at —
//!   ошибка) и текстом причины М-1 в relay.rs;
//! - (в) случай С-1: котировка-решение — итог ≥ 0; затем стакан хуже (тело стенда — тот же пример документации без
//!   двух верхних бидов; снимок REST забыт, как после ордера другой продажи — exec_book::forget) — продажа под ходом
//!   (sell_here_ok::try_sell) ордер не ставит; дальше продажа здесь по порогу — алерт, ордера нет.
//! - (г) «417 (ожидание)»: режим wait, тик монеты — «стало выгодно» по котировке-решению (первый запрос стакана); тест
//!   подменяет стакан на худший сразу после него; ответ баланса задержан стендом (300 мс), venue.book_stale_ms xt =
//!   200 мс — котировка под ходом идёт новым запросом стакана (второй) и видит худший стакан: ордера нет, ожидание
//!   продолжается (задача режима не закончена); по истечении stuck.wait_ms — как раньше, по рынку (ордер есть).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core (stuck, relay_here, sell_here_ok, relay_reserve, sell_turn_fresh, exec_book, ledger),
//! tests/t198_live.

mod t198_live;

use std::str::FromStr;
use std::time::Duration;

use arb::core::Core;
use arb::core::deal::{Deal, Step};
use arb::core::sell_here_ok::HereOk;
use arb::core::stuck_reason::StuckReason;
use arb::core::types::{Asset, Direction, Side, VenueId};
use arb::settings::value::Scope;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use t198_live::{Hit, Stand, big, start};

fn btc() -> Asset {
    Asset {
        ticker: "BTC".into(),
        bsc_contract: None,
    }
}

/// Сделка 1 BTC на xt (куплено на bsc_dex), вложено `invested`, режим застрявшей монеты.
fn deal(id: &str, invested: &str) -> Deal {
    let dir = Direction {
        asset: btc(),
        buy_on: VenueId("bsc_dex".into()),
        sell_on: VenueId("xt".into()),
    };
    let mut x = Deal::new(id.into(), dir, json!(null), Decimal::ZERO);
    x.at = VenueId("xt".into());
    x.qty = Decimal::ONE;
    x.invested = Decimal::from_str(invested).unwrap();
    x.step = Step::Stuck;
    x
}

/// Папка теста (новая).
fn dir(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("arb-417-live-{name}-{}", std::process::id()))
}

/// Общая настройка.
async fn set(core: &Core, name: &str, v: Value) {
    core.inner
        .deps
        .settings
        .set(name, Scope::Global, Some(v), "тест")
        .await
        .unwrap();
}

/// Число запросов вида `h` в журнале стенда.
fn count(stand: &Stand, h: Hit) -> usize {
    stand
        .hits
        .lock()
        .unwrap()
        .iter()
        .filter(|(x, _)| *x == h)
        .count()
}

/// Сделка из учёта ядра.
fn saved(core: &Core, id: &str) -> Deal {
    core.deals().into_iter().find(|d| d.id == id).unwrap()
}

/// Тело стенда хуже примера: тот же пример документации без двух верхних бидов (остаётся 20 × 10).
fn worse_book() -> String {
    let t = std::fs::read_to_string("tests/data/rest_book_2026-11-09/xt_depth_doc.json").unwrap();
    let mut v: Value = serde_json::from_str(&t).unwrap();
    let bids = v["result"]["bids"].as_array().unwrap()[2..].to_vec();
    v["result"]["bids"] = Value::Array(bids);
    v.to_string()
}

/// (а) Итог здесь ≥ 0: продажа здесь, вывода нет, алерта о незаданном пороге нет.
#[test]
fn profitable_here_sells_here_without_relay() {
    big(async {
        let (core, stand) = start(&dir("a"), "1").await;
        set(&core, "stuck.mode", json!("relay")).await;
        set(&core, "stuck.max_relays", json!(1)).await;
        let x = deal("T417A", "100");
        arb::core::ledger::save(&core, &x).await;
        let run = arb::core::stuck::handle_as_is(&core, x, StuckReason::Unprofitable);
        tokio::time::timeout(Duration::from_secs(30), run)
            .await
            .expect("режим закончился");
        let y = saved(&core, "T417A");
        assert!(
            count(&stand, Hit::Post) >= 1,
            "ордер здесь: {:?}",
            y.timeline
        );
        assert_eq!(count(&stand, Hit::Withdraw), 0, "вывода нет");
        assert!(
            y.timeline
                .iter()
                .any(|(_, t)| t.starts_with("Котировка продажи на xt")),
            "{:?}",
            y.timeline
        );
        assert!(
            !y.timeline
                .iter()
                .any(|(_, t)| t.contains("не задана настройка stuck.relay_max_loss_pct")),
            "{:?}",
            y.timeline
        );
    });
}

/// (б) Итог < 0: продажа здесь по порогу — убыток сверх порога, алерт; ордера и вывода нет.
#[test]
fn unprofitable_here_goes_by_threshold() {
    big(async {
        let (core, stand) = start(&dir("b"), "1").await;
        set(&core, "stuck.mode", json!("relay")).await;
        set(&core, "stuck.max_relays", json!(1)).await;
        set(&core, "stuck.relay_max_loss_pct", json!("1")).await;
        let x = deal("T417B", "1000");
        arb::core::ledger::save(&core, &x).await;
        let run = arb::core::stuck::handle_as_is(&core, x, StuckReason::Unprofitable);
        tokio::time::timeout(Duration::from_secs(30), run)
            .await
            .expect("режим закончился");
        let y = saved(&core, "T417B");
        assert_eq!(y.step, Step::WaitingHuman, "{:?}", y.timeline);
        assert_eq!(count(&stand, Hit::Post), 0, "ордера нет");
        assert_eq!(count(&stand, Hit::Withdraw), 0, "вывода нет");
        assert!(
            y.timeline
                .iter()
                .any(|(_, t)| t.contains("stuck.relay_max_loss_pct")),
            "{:?}",
            y.timeline
        );
    });
}

/// (в) С-1: между решением и ордером стакан хуже — по рынку в минус не продаёт.
#[test]
fn worse_book_under_turn_does_not_sell() {
    big(async {
        let (core, stand) = start(&dir("c"), "1").await;
        set(&core, "stuck.relay_max_loss_pct", json!("1")).await;
        let x = deal("T417C", "100");
        arb::core::ledger::save(&core, &x).await;
        // Котировка-решение (без хода) — итог ≥ 0.
        let (v, _) = arb::core::sell_turn_fresh::value(&core, &x).await.unwrap();
        assert!(arb::core::relay_reserve::here_first(&x, Some(v)), "{v}");
        // Стакан хуже; снимок REST забыт, как после ордера другой продажи (решение 238, п. 1).
        *stand.depth.lock().unwrap() = Some(worse_book());
        arb::core::exec_book::forget(&core, &VenueId("xt".into()), &btc(), Side::Sell);
        let back = match arb::core::sell_here_ok::try_sell(&core, x, "тест С-1").await {
            HereOk::NotNow(d, Some(v2)) => {
                assert!(!arb::core::sell::profitable(&d, v2), "{v2}");
                *d
            }
            HereOk::NotNow(_, None) => panic!("котировка под ходом не получена"),
            HereOk::Sold | HereOk::Gone => panic!("продал или сделки нет"),
        };
        assert_eq!(count(&stand, Hit::Post), 0, "ордера в минус нет");
        // Дальше — продажа здесь по порогу: убыток сверх 1 % — алерт, ордера нет.
        let run = arb::core::relay_here::sell_here(&core, back, "тест С-1");
        tokio::time::timeout(Duration::from_secs(30), run)
            .await
            .expect("продажа здесь закончилась");
        let y = saved(&core, "T417C");
        assert_eq!(y.step, Step::WaitingHuman, "{:?}", y.timeline);
        assert_eq!(count(&stand, Hit::Post), 0, "ордера нет");
    });
}

/// (г) «Ожидание»: стало выгодно по котировке-решению, под ходом стакан хуже — ордера нет, ожидание продолжается.
#[test]
fn wait_profitable_then_worse_keeps_waiting() {
    big(async {
        let (core, stand) = start(&dir("d"), "1").await;
        set(&core, "stuck.mode", json!("wait")).await;
        set(&core, "stuck.wait_ms", json!(3000)).await;
        let xt = Scope::Venue("xt".into());
        let s = &core.inner.deps.settings;
        s.set("venue.book_stale_ms", xt, Some(json!(200)), "тест")
            .await
            .unwrap();
        *stand.delay_ms.lock().unwrap() = 300;
        let x = deal("T417D", "100");
        arb::core::ledger::save(&core, &x).await;
        let c = core.clone();
        let task = tokio::spawn(async move {
            arb::core::stuck::handle_as_is(&c, x, StuckReason::Unprofitable).await
        });
        // Тик монеты — «Ожидание» котирует (котировка-решение).
        tokio::time::sleep(Duration::from_millis(100)).await;
        let _ = core.inner.coin_ticks.send(btc());
        let st = &stand;
        let wait_hits = move |n: usize| async move {
            for _ in 0..500 {
                if count(st, Hit::Depth) >= n {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            panic!("запросов стакана меньше {n}");
        };
        wait_hits(1).await;
        *stand.depth.lock().unwrap() = Some(worse_book());
        wait_hits(2).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(
            count(&stand, Hit::Post),
            0,
            "под ходом итог < 0 — ордера нет"
        );
        assert!(!task.is_finished(), "ожидание продолжается");
        // Срок вышел — как раньше: по рынку без порога (ордер есть).
        tokio::time::timeout(Duration::from_secs(30), task)
            .await
            .expect("режим закончился")
            .unwrap();
        assert!(
            count(&stand, Hit::Post) >= 1,
            "по истечении срока — по рынку"
        );
    });
}
