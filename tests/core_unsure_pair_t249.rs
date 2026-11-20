//! Файл: tests/core_unsure_pair_t249.rs
//! Что делает: решение 480 (ревью 3, задача t249) — пара котировок продажи биржи для маржинального счёта из ОДНОГО
//! снимка стакана (core/ride_pair.rs from_snapshot) на записанном стакане Bybit CAKEUSDT 04.11
//! (tests/data/cex_samples_2026_11_04/bybit_orderbook_CAKEUSDT.json, GET /v5/market/orderbook): наша часть после сдвига
//! (ride_quote::marginal) — ровно уровни стакана после первых «сдвиг» монет (независимый проход по уровням),
//! комиссия — от нашей суммы (ставка теста TEST_RATE — не из API, см. её комментарий), средняя хуже котировки без
//! сдвига (худший случай консервативен).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::ride_pair, arb::core::ride_quote, arb::gateway::frozen, tests/data.

use std::str::FromStr;

use arb::core::types::{Asset, OrderBook, PriceLevel, Side, VenueId};
use arb::core::{ride_pair, ride_quote};
use arb::gateway::frozen::{BookFee, FrozenBook, FrozenQuote as _};
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::Value;

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

/// Ставка тейкера ТЕСТА, не из ответа API: у Bybit ставка — своя у счёта, её отдаёт приватный GET /v5/account/fee-rate
/// (официальная документация «Get Fee Rate»: https://bybit-exchange.github.io/docs/v5/account/fee-rate — по ней
/// написан venues/bybit/fees.rs); записанного ответа в tests/data нет. Проверка от ставки не зависит: комиссия нашей
/// части — разность комиссий двух котировок одного снимка, должна быть ровно «ставка × наша сумма».
const TEST_RATE: &str = "0.001";

/// Снимок продажи по записанному стакану; комиссия — TEST_RATE от суммы в USDT (план «в получаемой монете», решение 232).
fn frozen() -> FrozenBook {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/cex_samples_2026_11_04/bybit_orderbook_CAKEUSDT.json"
    );
    let v: Value = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
    let lv = |k: &str| -> Vec<PriceLevel> {
        v["result"][k]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| PriceLevel {
                price: d(l[0].as_str().unwrap()),
                qty: d(l[1].as_str().unwrap()),
            })
            .collect()
    };
    let asset = Asset {
        ticker: "CAKE".into(),
        bsc_contract: None,
    };
    let venue = VenueId("bybit".into());
    FrozenBook {
        venue: venue.clone(),
        asset: asset.clone(),
        side: Side::Sell,
        book: OrderBook {
            venue,
            asset,
            bids: lv("b"),
            asks: lv("a"),
            exchange_time: None,
            received_at: Utc::now(),
        },
        fee: BookFee::in_received(d(TEST_RATE), Side::Sell, "CAKE", "USDT"),
        taken_at: Utc::now(),
    }
}

/// Сумма наших монет по уровням после первых `off` монет (независимо от FrozenBook). Ошибок нет.
fn after_off(levels: &[PriceLevel], off: Decimal, qty: Decimal) -> Decimal {
    let (mut skip, mut left, mut sum) = (off, qty, Decimal::ZERO);
    for l in levels {
        let mut here = l.qty;
        let s = skip.min(here);
        skip -= s;
        here -= s;
        let take = left.min(here);
        sum += take * l.price;
        left -= take;
    }
    assert!(left.is_zero(), "глубины записи хватает");
    sum
}

#[tokio::test]
async fn cex_pair_from_one_snapshot_is_levels_after_shift() {
    let fb = frozen();
    // Сдвиг 30 CAKE (остаток сделки с неизвестным исходом) съедает 1-й уровень и часть 2-го; наш объём 50.
    let (off, qty) = (d("30"), d("50"));
    let (t, h) = ride_pair::from_snapshot(&fb, qty, off).await.unwrap();
    assert_eq!((t.fillable_qty, h.fillable_qty), (off + qty, off));
    let m = ride_quote::marginal(&t, &h, qty, off);
    assert_eq!(m.fillable_qty, qty);
    let ours = after_off(&fb.book.bids, off, qty);
    let gap = (m.avg_price * qty - ours).abs();
    assert!(gap < Decimal::new(1, 18), "{} vs {ours}", m.avg_price * qty);
    // Комиссия — от нашей суммы (разность комиссий двух котировок одного снимка).
    let fee: Decimal = m.costs.iter().map(|c| c.amount).sum();
    assert_eq!(fee, ours * d(TEST_RATE));
    // Худший случай консервативен: средняя после сдвига хуже котировки без сдвига.
    let plain = fb.quote(qty).await.unwrap();
    assert!(m.avg_price < plain.avg_price, "{} < {}", m.avg_price, plain.avg_price);
}
