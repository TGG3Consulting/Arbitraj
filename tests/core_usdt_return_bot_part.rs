//! Файл: tests/core_usdt_return_bot_part.rs
//! Что делает: решение 224 (уточнение к решению 100) на настоящей SQLite без площадок в реестре (моков нет). Живой
//! случай 08.11 (20261108102353547PIT, mexc → DEX, кошелёк w2): бот сам продал 290254595304.96 PIT на DEX (≈44.93 USDT),
//! фантомный остаток человек закрыл «Продано вручную» (0). Проверяется: (1) чистая логика — выручка продаж бота на
//! площадке выручки минус их комиссия в котируемой монете; продажи на другой площадке не считаются; (2) сделка с
//! продажей бота + «Продано вручную» — бот начинает возврат именно суммы бота (ручная выручка не перегоняется);
//! после неудачи «Перегнать» доступна и текст её предлагает; (3) полностью ручная — возврата нет, «Перегнать»
//! недоступна и текст записи её не предлагает (только «Перегнано вручную»).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::settings, arb::storage, arb::notify, arb::secrets.

mod usdt_return_common;

use std::str::FromStr;
use std::time::Duration;

use arb::core::Core;
use arb::core::deal::{Deal, Step};
use arb::core::types::{
    CommandSource, DexAccountId, Fee, ManualAction, ManualCommand, Side, TradeRequest, TradeResult,
    TradeStatus, VenueId,
};
use arb::core::usdt_return_deal::{bot_part, bot_sold};
use arb::core::usdt_return_state::UsdtReturn;
use rust_decimal::Decimal;
use usdt_return_common::{dir, doge, entries, start, wait_state};

/// Число из строки. Ошибок нет (тестовые литералы).
fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

/// Продажа бота: qty по avg на площадке venue, комиссия fees. Ошибок нет.
fn sell(venue: &str, qty: Decimal, avg: Decimal, fees: Vec<Fee>) -> TradeResult {
    TradeResult {
        request: TradeRequest {
            venue: VenueId(venue.into()),
            asset: doge("x").direction.asset,
            side: Side::Sell,
            qty,
            worst_price: avg,
            deal_id: "x".into(),
            route: None,
            exit_position: true,
        },
        status: TradeStatus::Filled,
        filled_qty: qty,
        avg_price: avg,
        fees,
        venue_ref: "0x".into(),
        chain: None,
        finished_at: chrono::Utc::now(),
        executor: None,
    }
}

/// Сделка как живая PIT: куплено на mexc, бот продал на DEX (кошелёк w2) 290254595304.96 по 0.0000000001548
/// (≈44.93 USDT), в сделке остался фантомный остаток; остановлена. bot — была ли продажа бота. Ошибок нет.
fn pit(id: &str, bot: bool) -> Deal {
    let mut x = doge(id);
    x.direction.buy_on = VenueId("mexc".into());
    x.direction.sell_on = VenueId("bsc_dex".into());
    x.at = VenueId("bsc_dex".into());
    x.dex_account = Some(DexAccountId("w2".into()));
    x.invested = Decimal::from(45);
    x.proceeds = Decimal::ZERO;
    x.qty = d("1000");
    if bot {
        let r = sell(
            "bsc_dex",
            d("290254595304.96"),
            d("0.0000000001548"),
            vec![],
        );
        x.proceeds = r.filled_qty * r.avg_price;
        x.sells.push(r);
    }
    x.step = Step::Halted;
    x
}

/// «Продано вручную» из интерфейса. Возвращает итог ядра.
async fn sold_manually(core: &Core, id: &str, usdt: &str) -> Result<String, String> {
    core.command(ManualCommand {
        action: ManualAction::SoldManually {
            deal_id: id.into(),
            usdt: usdt.into(),
        },
        source: CommandSource::Ui,
        by: Some("тест".into()),
        at: chrono::Utc::now(),
    })
    .await
}

#[test]
fn bot_part_counts_only_bot_sells_at_proceeds_venue() {
    let mut x = doge("p1DOGE");
    let fee = Fee {
        asset: "USDT".into(),
        amount: d("0.1"),
        usd: None,
    };
    x.sells.push(sell("mexc", d("100"), d("0.5"), vec![fee]));
    // Продажа на другой площадке — её выручка не на mexc, в возврат с mexc не входит.
    x.sells.push(sell("bingx", d("10"), d("0.5"), vec![]));
    assert_eq!(bot_sold(&x), d("50"));
    assert_eq!(bot_part(&x, Some("USDT")), d("49.9"));
    assert_eq!(bot_part(&x, None), d("50"));
    // Полностью ручная (продаж бота нет) — 0.
    assert_eq!(bot_sold(&doge("p2DOGE")), Decimal::ZERO);
}

#[tokio::test]
async fn bot_sell_plus_sold_manually_returns_bot_part() {
    let (core, j) = start(&dir("b224bot"), None).await;
    let x = pit("b1PIT", true);
    let bot = x.proceeds;
    arb::core::ledger::save(&core, &x).await;
    sold_manually(&core, "b1PIT", "0").await.unwrap();
    // Возврат начат на сумму продаж бота (комиссии в USDT у свопа нет) — журнал «возврат начат».
    let e = entries(&j, "core.usdt_return", "b1PIT: возврат USDT").await;
    assert_eq!(e.len(), 1, "{e:?}");
    assert!(e[0].message.contains(&bot.to_string()), "{}", e[0].message);
    // Площадок нет — перегон не состоялся: «не вернулись» на сумму бота; «Перегнать» доступна и предлагается.
    let s = wait_state(&core, "b1PIT", Duration::from_secs(5), |s| {
        matches!(s, UsdtReturn::NotReturned { .. })
    })
    .await;
    match s {
        Some(UsdtReturn::NotReturned { amount, .. }) => assert_eq!(amount, bot),
        other => panic!("ждали «не вернулись», а {other:?}"),
    }
    let rec = core
        .manual_records()
        .into_iter()
        .find(|r| r.key == "u.b1PIT")
        .unwrap();
    assert!(rec.button("usdt_retry").unwrap().disabled.is_none());
    assert!(rec.text.contains("«Перегнать» или"), "{}", rec.text);
}

#[tokio::test]
async fn fully_manual_has_no_return_and_text_without_retry() {
    let (core, j) = start(&dir("b224man"), None).await;
    arb::core::ledger::save(&core, &pit("b2PIT", false)).await;
    sold_manually(&core, "b2PIT", "44.93").await.unwrap();
    let x = core.deals().into_iter().find(|d| d.id == "b2PIT").unwrap();
    match &x.usdt_return {
        Some(UsdtReturn::NotReturned { amount, why, .. }) => {
            assert_eq!(*amount, d("44.93"));
            assert!(why.contains("решение 100"), "{why}");
        }
        other => panic!("ждали «не вернулись», а {other:?}"),
    }
    assert!(entries(&j, "core.usdt_return", "b2PIT").await.is_empty());
    let rec = core
        .manual_records()
        .into_iter()
        .find(|r| r.key == "u.b2PIT")
        .unwrap();
    assert!(rec.button("usdt_retry").unwrap().disabled.is_some());
    // Решение 224: недоступную «Перегнать» текст не предлагает — только «Перегнано вручную».
    assert!(!rec.text.contains("«Перегнать»"), "{}", rec.text);
    assert!(rec.text.contains("«Перегнано вручную»"), "{}", rec.text);
}
