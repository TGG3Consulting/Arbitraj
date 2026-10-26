//! Файл: tests/bsc_dex_config_wake.rs
//! Что делает: «не задана настройка — задали на лету — работа пошла сразу» (CLAUDE.md: настройки меняются на лету без
//! перезапуска). Живой бот 26.10 (main 0c965d8): бот запущен без dex.depth_band_pct, проба налогов дала «не задана
//! настройка» (Why::Config), настройку задали в 11:06:18, а 313 монет ждали следующего полного прохода
//! tax.recheck_interval_s (час). На живом пуле AAVE Uniswap V3 0x8e3ecc0b… (запись 2026-10-25):
//! - без dex.depth_band_pct отбор пулов пробы — «не задана настройка» (как у бота), монета — Why::Config;
//! - изменение настроек сразу выдаёт её на проверку (tax_wait::due_config по событию изменения), без полного прохода;
//! - после задания отбор пулов проходит;
//! - цикл, которому нужна настройка (интервал прохода), не останавливается, а ждёт её и продолжает (settings_wait).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/native_common, tests/depth_common, arb::venues::bsc_dex::{parts, settings_wait,
//! check::{tax_wait, taxes_util}}.

mod depth_common;
mod native_common;

use std::collections::HashMap;
use std::time::Duration;

use serde_json::json;

use arb::core::types::Asset;
use arb::settings::read::At;
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::check::tax_pass::Why;
use arb::venues::bsc_dex::check::tax_wait::due_config;
use arb::venues::bsc_dex::check::taxes_util::probe_pools;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::settings_wait;

use depth_common::{aave_v3, rec};

#[tokio::test]
async fn config_wait_rechecks_on_settings_change() {
    let ctx = native_common::ctx("config-wake").await;
    let v = Scope::Venue("bsc_dex".into());
    for (n, x) in [
        ("venue.quote_asset", json!("USDT")),
        ("dex.depth_hysteresis_pct", json!(10)),
        ("dex.min_pool_liquidity_usd", json!(100)),
    ] {
        ctx.settings
            .set(n, v.clone(), Some(x), "тест")
            .await
            .unwrap();
    }
    let p = Parts::new(ctx.clone());
    let usdt = alloy_primitives::address!("0x55d398326f99059ff775485246999027b3197955");
    p.tokens.set(&[Asset {
        ticker: "USDT".into(),
        bsc_contract: Some(usdt),
    }]);
    let (e, _, _) = aave_v3(&rec());
    p.registry.put(e.clone());
    // Как у бота: полоса не задана — «не задана настройка», монета ждёт её (Why::Config).
    let err = probe_pools(&ctx, &p.registry, &p.tokens, "AAVE", 18).unwrap_err();
    assert!(
        err.message.contains("dex.depth_band_pct"),
        "{}",
        err.message
    );
    // Имя настройки — в ошибке структурно (GatewayError.setting), по нему монета ждёт именно её.
    assert_eq!(err.setting.as_deref(), Some("dex.depth_band_pct"));
    let names = HashMap::from([(e.coin, vec!["dex.depth_band_pct".to_string()])]);
    let waiting = vec![
        (e.coin, "AAVE".to_string(), Why::Config),
        (usdt, "X".into(), Why::Node),
    ];
    // Подписка — как у цикла налогов, до изменения.
    let mut changes = settings_wait::watch(&ctx);
    // Ревью #65: 30 изменений посторонних настроек подряд — один проход и ни одной перепроверки.
    for k in 0..30u64 {
        ctx.settings
            .set("dex.load_batch", v.clone(), Some(json!(100 + k)), "Тигран")
            .await
            .unwrap();
    }
    let first = tokio::time::timeout(Duration::from_secs(2), changes.next())
        .await
        .unwrap()
        .unwrap();
    let changed = settings_wait::drain(first, &mut changes);
    assert_eq!(changed.len(), 1, "пачка схлопнута: {changed:?}");
    assert!(
        due_config(&waiting, &names, &changed).is_empty(),
        "посторонние — без перепроверки"
    );
    let c2 = ctx.clone();
    let v2 = v.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        c2.settings
            .set("dex.depth_band_pct", v2, Some(json!(2)), "Тигран")
            .await
            .unwrap();
    });
    let got = tokio::time::timeout(Duration::from_secs(2), changes.next()).await;
    let Ok(Some(c)) = got else {
        panic!("изменение настроек пришло сразу, без полного прохода");
    };
    let changed = settings_wait::drain(c, &mut changes);
    assert_eq!(
        due_config(&waiting, &names, &changed),
        vec![(e.coin, "AAVE".to_string())],
        "нужная — одна перепроверка, только ждущей её монеты"
    );
    // Повтор: отбор пулов пробы проходит.
    let (pools, _, _) = probe_pools(&ctx, &p.registry, &p.tokens, "AAVE", 18).unwrap();
    assert_eq!(pools.len(), 1);
}

#[tokio::test]
async fn cycle_waits_for_setting_instead_of_stopping() {
    let ctx = native_common::ctx("config-wake-cycle").await;
    let c2 = ctx.clone();
    let wait = tokio::spawn(async move {
        let read = || c2.settings.integer("tax.recheck_interval_s", At::default());
        settings_wait::until_ok(&c2, "Цикл налогов", "dex.check", read).await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !wait.is_finished(),
        "настройки нет — цикл ждёт, а не выходит"
    );
    ctx.settings
        .set(
            "tax.recheck_interval_s",
            Scope::Global,
            Some(json!(3600)),
            "Тигран",
        )
        .await
        .unwrap();
    let got = tokio::time::timeout(Duration::from_secs(2), wait)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got, Some(3600), "задали — продолжил сам");
}
