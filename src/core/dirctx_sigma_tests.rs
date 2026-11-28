//! Файл: core/dirctx_sigma_tests.rs
//! Что проверяет: круг «цены нет без σ — σ нет без замеров — замеров нет без цены» у денежного круга конверсии снят
//! (найдено в живом боте 28.11.2026, сборка e9907a2f). Новая сеть без истории, направление gate → base_dex:
//! 1) ранний сигнал (Moment::Signal, settlement_model::signal) σ не требует — 0; вход (Moment::Entry) честно отвечает
//!    «волатильность … ещё не измерена»;
//! 2) замеры цен сигнала (evaluate.rs sample — единственная запись рядов σ) с первого тика копятся в ТОТ ЖЕ ряд, который
//!    читает вход (продажа на B, напарник A, решение 118) — после N тиков σ входа появляется и равна σ ряда vol.rs.
//!
//! Цены продажи на B — записанные лучшие биды Gate CAKE_USDT (tests/data/cex_samples_2026_11_04/gate_ws.jsonl); время
//! тиков — последние N секунд до «сейчас» (вход меряет окно от текущего времени). Моков нет: ядро на временной SQLite.
//! Кто вызывает: cargo test.
//! От чего зависит: dirctx.rs (sigma_at, Moment), evaluate.rs (sample), vol.rs, test_core.rs.

use std::str::FromStr;

use chrono::{Duration, Utc};
use rust_decimal::Decimal;
use serde_json::{Value, json};

use super::{Moment, sigma_at};
use crate::core::engine::lk;
use crate::core::signal::Prices;
use crate::core::signal_types::VenueSale;
use crate::core::types::{Asset, Direction, Side, VenueId};
use crate::settings::value::Scope;

/// Записанные лучшие биды Gate CAKE_USDT по кадрам spot.order_book «update».
fn gate_bids() -> Vec<Decimal> {
    include_str!("../../tests/data/cex_samples_2026_11_04/gate_ws.jsonl")
        .lines()
        .filter_map(|l| {
            let o: Value = serde_json::from_str(l).ok()?;
            let m: Value = serde_json::from_str(o["msg"].as_str()?).ok()?;
            (m["channel"] == "spot.order_book" && m["event"] == "update")
                .then(|| Decimal::from_str(m["result"]["bids"][0][0].as_str()?).ok())
                .flatten()
        })
        .collect()
}

fn dir() -> Direction {
    Direction {
        asset: Asset {
            chain: None,
            ticker: "CAKE".into(),
            contract: None,
        },
        buy_on: VenueId("gate".into()),
        sell_on: VenueId("base_dex".into()),
    }
}

#[tokio::test]
async fn new_network_gets_sigma_from_signal_prices() {
    let path = std::env::temp_dir().join(format!(
        "arb-settle-sigma-{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap()
    ));
    std::fs::create_dir_all(&path).unwrap();
    let core = crate::core::test_core::open(&path).await;
    let s = core.inner.deps.settings.clone();
    s.set(
        "profit.sigma_window_min",
        Scope::Global,
        Some(json!(60)),
        "тест",
    )
    .await
    .unwrap();
    let d = dir();
    // 1) Новая сеть: рядов нет. Сигнал σ не ждёт, вход — честный отказ.
    assert_eq!(
        sigma_at(&core, &s, &d, &Moment::Signal).unwrap(),
        Decimal::ZERO
    );
    let e = sigma_at(&core, &s, &d, &Moment::Entry).unwrap_err();
    assert!(e.contains("ещё не измерена"), "{e}");
    // 2) N тиков сигнала: цена покупки на A и продажи на B (единицы B — venue_sale, как у модели круга).
    let bids = gate_bids();
    assert!(bids.len() >= 3, "в записи есть кадры стакана");
    assert!(
        bids.windows(2).any(|w| w[0] != w[1]),
        "цена в записи меняется"
    );
    let n = bids.len() as i64;
    let start = Utc::now() - Duration::seconds(n + 1);
    for (i, bid) in bids.iter().enumerate() {
        let at = start + Duration::seconds(i as i64);
        let p = Prices {
            buy: *bid,
            sell: *bid,
            venue_sale: Some(VenueSale {
                price: *bid,
                qty: Decimal::ONE,
            }),
            fees_pct: Decimal::ZERO,
            qty: Decimal::ONE,
            buy_at: at,
            sell_at: at,
        };
        crate::core::evaluate::sample(&core, &d, &p, at);
        if i == 0 {
            // Один замер — шагов нет: вход по-прежнему отказывает (не торгуем вслепую).
            assert!(sigma_at(&core, &s, &d, &Moment::Entry).is_err());
        }
    }
    let sigma = sigma_at(&core, &s, &d, &Moment::Entry).unwrap();
    assert!(sigma > Decimal::ZERO);
    let series = lk(&core.inner.vol)
        .sigma(&d.sell_on, &d.asset, Side::Sell, &d.buy_on, 60, Utc::now())
        .unwrap();
    assert_eq!(sigma, series);
    // Сигнал по-прежнему без σ (запас — только у входа).
    assert_eq!(
        sigma_at(&core, &s, &d, &Moment::Signal).unwrap(),
        Decimal::ZERO
    );
}
