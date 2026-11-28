//! Записанные времена ставок издателя. Время прохождения обычной ноги задают виртуальные часы Tokio.
use super::*;
use serde_json::Value;

fn captures() -> Value {
    serde_json::from_str(include_str!(
        "../../../../tests/data/first_run_dex_2026-11-26/tokenlon_ethereum_additional_rates.json"
    ))
    .unwrap()
}

#[tokio::test(start_paused = true)]
async fn late_ordinary_route_preserves_time_for_second_leg() {
    let capture = captures();
    let start = Instant::now();
    let ms = Duration::from_millis;
    let outer = start + ms(2000);
    let budget = ms(1000);
    let remaining = fallback_deadline(start + budget, budget, Some(outer));
    tokio::time::sleep_until(start + ms(1500)).await;
    let now = Instant::now();
    for rate in capture["rates"].as_array().unwrap() {
        let latency = ms(rate["elapsed_ms"].as_u64().unwrap());
        let delayed = async {
            tokio::time::sleep(latency).await;
            rate
        };
        assert!(until_ready(remaining, delayed).await.is_none());
        assert_eq!(Instant::now(), now);
        assert!(
            until_ready(remaining, std::future::ready(rate))
                .await
                .is_some()
        );
    }
    assert!(Instant::now() < outer);
}

#[tokio::test(start_paused = true)]
async fn strong_quote_inside_competition_window_survives_and_slow_quote_is_cancelled() {
    let capture = captures();
    let budget = Duration::from_millis(1000);
    for rate in capture["rates"].as_array().unwrap() {
        let latency = Duration::from_millis(rate["elapsed_ms"].as_u64().unwrap());
        if latency == budget {
            continue;
        }
        let start = Instant::now();
        let window = fallback_deadline(start + budget, budget, Some(start + budget + budget));
        let offer = async {
            tokio::time::sleep(latency).await;
            rate
        };
        let result = until_ready(window, offer).await;
        assert_eq!(result.is_some(), latency < budget);
        assert!(Instant::now() - start >= latency.min(budget));
        assert!(Instant::now() < start + budget + budget);
    }
}

#[tokio::test(start_paused = true)]
async fn race_keeps_ready_ordinary_and_compares_firm_both_completion_orders() {
    let capture = captures();
    let ms = Duration::from_millis;
    let observed = capture["rates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["pair"] == "WBTC_USDT" && r["side"] == "SELL")
        .unwrap();
    let latency = ms(observed["elapsed_ms"].as_u64().unwrap());
    let budget = ms(1000);
    for (before, ordinary_time, expect_firm) in [
        (ms(1500), ms(0), false),
        (ms(0), ms(100), true),
        (ms(0), ms(800), true),
    ] {
        let start = Instant::now();
        let outer = start + ms(2000);
        tokio::time::sleep_until(start + before).await;
        let end = Instant::now() + budget;
        let ordinary = async {
            tokio::time::sleep(ordinary_time).await;
            "DEX"
        };
        let offer = async {
            tokio::time::sleep(latency).await;
            Some(observed)
        };
        let (route, firm) = competition(ordinary, offer, end, |_| {
            Window::Until(fallback_deadline(end, budget, Some(outer)).unwrap())
        })
        .await;
        assert_eq!(route, "DEX");
        assert_eq!(firm.is_some(), expect_firm);
        assert!(Instant::now() < outer);
    }
}

#[test]
fn archived_wbtc_fee_and_usdt_budget_use_distinct_assets_and_round_once() {
    use crate::venues::bsc_dex::tokenlon::wire::abi::FillOrder;
    use alloy::{rpc::types::Log, sol_types::SolEvent};
    use flate2::read::GzDecoder;
    let data = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/tokenlon_ethereum_expired_pmm.json.gz"
    );
    let snapshot: Value = serde_json::from_reader(GzDecoder::new(&data[..])).unwrap();
    let mut checked = false;
    for row in snapshot["rows"].as_array().unwrap() {
        for value in row["logs"].as_array().unwrap() {
            let log = Log {
                inner: serde_json::from_value(value.clone()).unwrap(),
                ..Default::default()
            };
            if log.topics().first() != Some(&FillOrder::SIGNATURE_HASH) {
                continue;
            }
            let pay = FillOrder::decode_log_data_validate(&log.inner.data).unwrap();
            if pay.makerAssetAddr
                != alloy_primitives::address!("2260fac5e5542a773aa44fbcfedf7c193bc2c599")
                || pay.takerAssetAddr
                    != alloy_primitives::address!("dac17f958d2ee523a2206206994597c13d831ec7")
                || pay.makerAssetAmount != U256::from(50050000u64)
            {
                continue;
            }
            let net = pay.makerAssetAmount * U256::from(10000 - pay.feeFactor) / U256::from(10000);
            assert_eq!(net, U256::from(49999950u64));
            assert_eq!(
                gross_for_net(pay.makerAssetAmount, pay.feeFactor),
                Some(U256::from(50100101u64))
            );
            let cash = num::to_decimal(pay.takerAssetAmount, 6).unwrap();
            assert!(fits_principal(pay.takerAssetAmount, 6, Some(cash)));
            assert!(!fits_principal(
                pay.takerAssetAmount,
                6,
                Some(cash - Decimal::new(1, 6))
            ));
            checked = true;
        }
    }
    assert!(checked);
}
