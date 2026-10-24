//! Файл: tests/bingx_transfer_status.rs
//! Что делает: живой случай 2026-10-24 12:42 — справочник сетей BingX за один опрос показал «ввод закрыт, вывод
//! закрыт» по 100 монетам, ушло 100 сообщений. Причина массового «закрыт» не установлена: сырых ответов BingX за то
//! время нет, работал старый код с unwrap_or(false). Проверяется:
//! 1) флаг сети — только boolean (официальная таблица полей config/getall: depositEnable | bool); нет поля или другой
//!    тип — ошибка BadResponse, а не «закрыт» по догадке;
//! 2) массовая смена при заданном venue.status_bulk_min — одна сводка с полным списком; меньше порога — по монете.
//!
//! Монеты — реальные тикеры BingX из журнала того же запуска (tests/data/live_2026-10-24/dex_subscription.json,
//! запись «нет торгуемой пары»). Ответ config/getall в записях нет (сырой журнал REST появляется этим PR), поэтому
//! элемент networkList собран по официальной таблице полей.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bingx::wallet, arb::core::transfer_status. Моков нет.

use std::collections::HashMap;

use alloy_primitives::Address;
use chrono::Utc;
use serde_json::{Value, json};

use arb::core::transfer_status::{self, Change};
use arb::core::types::{Asset, CoinNetworkInfo, VenueId};
use arb::gateway::errors::ErrorCode;
use arb::venues::bingx::wallet::flag;

/// Реальные тикеры BingX из журнала запуска 2026-10-24.
fn tickers() -> Vec<String> {
    let raw = std::fs::read_to_string("tests/data/live_2026-10-24/dex_subscription.json").unwrap();
    let fx: Value = serde_json::from_str(&raw).unwrap();
    let msg = fx["journal_bingx_no_pair"]["message"].as_str().unwrap();
    msg.split_once(": ")
        .unwrap()
        .1
        .split(", ")
        .map(String::from)
        .collect()
}

fn net(ticker: &str, open: bool) -> CoinNetworkInfo {
    CoinNetworkInfo {
        venue: VenueId("bingx".into()),
        asset: Asset {
            ticker: ticker.into(),
            bsc_contract: Some(Address::ZERO),
        },
        network: "BEP20".into(),
        deposit_enabled: open,
        withdraw_enabled: open,
        withdraw_fee: None,
        withdraw_min: None,
        withdraw_max: None,
        deposit_min: None,
        deposit_confirmations: None,
        updated_at: Utc::now(),
    }
}

#[test]
fn network_flag_is_strict_boolean() {
    let ok = json!({"network": "BEP20", "depositEnable": true, "withdrawEnable": false});
    assert!(flag(&ok, "depositEnable", "CAKE").unwrap());
    assert!(!flag(&ok, "withdrawEnable", "CAKE").unwrap());
    for bad in [
        json!({"network": "BEP20"}),
        json!({"network": "BEP20", "depositEnable": "true"}),
        json!({"network": "BEP20", "depositEnable": null}),
    ] {
        let e = flag(&bad, "depositEnable", "CAKE").unwrap_err();
        assert_eq!(e.code, ErrorCode::BadResponse);
        assert!(e.message.contains("depositEnable"));
    }
}

#[test]
fn mass_change_is_one_summary() {
    let list = tickers();
    let hundred = &list[..100];
    let old: HashMap<(String, String), (bool, bool)> = hundred
        .iter()
        .map(|t| ((t.clone(), "BEP20".to_string()), (true, true)))
        .collect();
    let new: Vec<CoinNetworkInfo> = hundred.iter().map(|t| net(t, false)).collect();
    let changes = transfer_status::diff(&old, &new);
    assert_eq!(changes.len(), 100);
    assert!(transfer_status::bulk(changes.len(), Some(20)));
    let (text, data) = transfer_status::summary("bingx", &changes);
    assert!(text.contains("у 100 монет") && text.contains("ввод закрыт, вывод закрыт — 100"));
    assert_eq!(data.as_array().unwrap().len(), 100);
    // Порог не задан или смен меньше порога — по сообщению на монету.
    assert!(!transfer_status::bulk(100, None));
    assert!(!transfer_status::bulk(3, Some(20)));
    let c = Change {
        ticker: hundred[0].clone(),
        network: "BEP20".into(),
        deposit: false,
        withdraw: true,
    };
    assert_eq!(
        transfer_status::one("bingx", &c),
        format!("bingx {} (BEP20): ввод закрыт, вывод открыт", hundred[0])
    );
    // Монеты, которой не было в прошлом справочнике, — не смена.
    assert!(transfer_status::diff(&HashMap::new(), &new).is_empty());
}
