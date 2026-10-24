//! Файл: tests/core_subscribe_retry.rs
//! Что делает: подписка площадки после сбоя (core/sub_state.rs, core/rebuild.rs retry_on) — по живому случаю
//! 2026-10-24 12:30–12:35: подписка DEX падала с «Не задана настройка bsc.node_http_rps» на каждой пересборке
//! (215 записей Warn за 38 с), а после того как настройку задали, подписка не повторялась до перезапуска.
//! Проверяется: одна запись на причину; пока подписка идёт — вторая не запускается; изменение любой настройки при
//! неудавшейся подписке — толчок пересборки, без сбоев — только настройки справочника; после успеха тот же список
//! не переподписывается. Монеты — из живых данных (tests/data/live_2026-10-24/dex_subscription.json).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core (sub_state, rebuild). Моков нет.

use std::str::FromStr;

use alloy_primitives::Address;
use serde_json::Value;

use arb::core::rebuild::retry_on;
use arb::core::sub_state::{Note, SubState};
use arb::core::types::{Asset, VenueId};

/// Монеты подписки DEX из живой записи (тикер и адрес в BSC) плюс USDT.
fn assets() -> Vec<Asset> {
    let raw = std::fs::read_to_string("tests/data/live_2026-10-24/dex_subscription.json").unwrap();
    let fx: Value = serde_json::from_str(&raw).unwrap();
    let mut out: Vec<Asset> = fx["pools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| Asset {
            ticker: p["ticker"].as_str().unwrap().into(),
            bsc_contract: Some(Address::from_str(p["token"].as_str().unwrap()).unwrap()),
        })
        .collect();
    out.push(Asset {
        ticker: "USDT".into(),
        bsc_contract: Some(Address::from_str(fx["usdt"].as_str().unwrap()).unwrap()),
    });
    out
}

#[test]
fn failed_subscription_is_logged_once_and_retried_on_any_setting() {
    let dex = VenueId("bsc_dex".into());
    let want = assets();
    let why = "Не задана настройка bsc.node_http_rps — задайте её в интерфейсе".to_string();
    let mut s = SubState::default();
    // Пустой список до первой подписки — подписываться не на что.
    assert!(!s.begin(&dex, &[]));
    // 215 пересборок подряд с той же причиной — одна запись.
    let mut warns = 0;
    for _ in 0..215 {
        assert!(s.begin(&dex, &want), "после сбоя — повтор по толчку");
        if let Note::Failed(_) = s.done(&dex, want.clone(), Err(why.clone())) {
            warns += 1;
        }
    }
    assert_eq!(warns, 1);
    // Пока сбой — изменение любой настройки толкает пересборку (bsc.* справочник не смотрит).
    assert!(s.any_failed());
    assert!(retry_on("bsc.node_http_rps", s.any_failed()));
    assert!(retry_on("tax.retry_s", false));
    assert!(!retry_on("bsc.node_http_rps", false));
    // Подписка пошла (долго, поиск пулов) — пересборки в это время вторую не запускают.
    assert!(s.begin(&dex, &want));
    assert!(!s.begin(&dex, &want));
    assert_eq!(
        s.done(&dex, want.clone(), Ok(())),
        Note::Subscribed(want.len())
    );
    assert!(!s.any_failed());
    // Тот же список — без новой подписки; другой — новая.
    assert!(!s.begin(&dex, &want));
    assert!(s.begin(&dex, &want[..2]));
    // Новая причина сбоя — новая запись.
    assert!(matches!(
        s.done(
            &dex,
            want[..2].to_vec(),
            Err("узел BSC (HTTP): нет связи".into())
        ),
        Note::Failed(_)
    ));
}
