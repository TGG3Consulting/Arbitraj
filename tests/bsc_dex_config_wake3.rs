//! Файл: tests/bsc_dex_config_wake3.rs
//! Что делает: решение 75, ревью #65 (живой пул AAVE Uniswap V3 0x8e3ecc0b…, запись 2026-10-25):
//! - имя незаданной настройки у итога сверки с квотером — структурно (quoter_verify::missing_setting, из
//!   GatewayError.setting): сторона ждёт её и перепроверяется только при её изменении (QuoterSides::waits_for);
//! - монета, ждущая из-за отставания ленты (Why::Lag): отметка стоит — не выдаётся; сняли — выдаётся на проверку на
//!   ближайшем шаге цикла (due_lag), сигнал до постановки в ожидание не теряется; снятие будит «пул готов»;
//! - пересчёт глубины, которому нужен узел (pools/depth_sync.rs), «замораживает» пачку: пока чтение идёт, её пулы
//!   недостоверны и котировка их не берёт; после — снова достоверны, не прочитавшиеся — нет.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/native_common, tests/depth_common, arb::venues::bsc_dex::{parts, check::{quoter_verify,
//! quoter_sides, tax_wait}, node::events_lag, pools::depth_sync, quote::select}.

mod depth_common;
mod native_common;

use std::collections::HashSet;
use std::time::Duration;

use serde_json::json;

use arb::core::types::{Asset, Side};
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::check::quoter_sides::QuoterSides;
use arb::venues::bsc_dex::check::quoter_verify::missing_setting;
use arb::venues::bsc_dex::check::tax_pass::Why;
use arb::venues::bsc_dex::check::tax_wait::due_lag;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::pools::depth_sync::{freeze, thaw};
use arb::venues::bsc_dex::quote::select::usable;

use depth_common::{aave_v3, rec};

#[tokio::test]
async fn quoter_side_waits_for_named_setting() {
    let ctx = native_common::ctx("wake3-quoter").await;
    let v = Scope::Venue("bsc_dex".into());
    ctx.settings
        .set("venue.quote_asset", v.clone(), Some(json!("USDT")), "тест")
        .await
        .unwrap();
    let p = Parts::new(ctx.clone());
    let usdt = alloy_primitives::address!("0x55d398326f99059ff775485246999027b3197955");
    p.tokens.set(&[Asset {
        ticker: "USDT".into(),
        bsc_contract: Some(usdt),
    }]);
    let (e, _, _) = aave_v3(&rec());
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let pools = p.pools(tx);
    // Глубина не задана — первым ждётся порог.
    let name = missing_setting(&pools, &e).unwrap();
    assert_eq!(name, "dex.min_pool_liquidity_usd");
    let mut q = QuoterSides::default();
    q.set(Side::Buy, Some(Err("не задана".into())));
    q.set_wait(Side::Buy, Some(name.clone()));
    let other: HashSet<String> = (0..30).map(|k| format!("dex.other_{k}")).collect();
    assert!(!q.waits_for(&other), "посторонние — без перепроверки");
    assert!(q.waits_for(&HashSet::from([name])));
}

#[tokio::test]
async fn lag_wait_released_on_clear() {
    let ctx = native_common::ctx("wake3-lag").await;
    let p = Parts::new(ctx);
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let pools = p.pools(tx);
    let coin = alloy_primitives::address!("0xfb6115445bff7b52feb98650c87f44907e58f802");
    let waiting = vec![(coin, "AAVE".to_string(), Why::Lag)];
    pools.lag.set(15);
    assert!(
        due_lag(&waiting, pools.lag.busy()).is_empty(),
        "отметка стоит — ждёт"
    );
    let w = pools.signals.ready.notified();
    tokio::pin!(w);
    w.as_mut().enable();
    // Порог bsc.receipts_max_lag_blocks подняли — лента сняла отметку.
    pools.lag.set(0);
    assert!(
        tokio::time::timeout(Duration::from_millis(200), w)
            .await
            .is_ok(),
        "снятие будит «пул готов»"
    );
    assert_eq!(
        due_lag(&waiting, pools.lag.busy()),
        vec![(coin, "AAVE".to_string())]
    );
}

#[tokio::test]
async fn depth_sync_freezes_pack_while_reading() {
    let ctx = native_common::ctx("wake3-freeze").await;
    let v = Scope::Venue("bsc_dex".into());
    for (n, x) in [
        ("venue.quote_asset", json!("USDT")),
        ("dex.depth_band_pct", json!(2)),
        ("dex.depth_hysteresis_pct", json!(0)),
        ("dex.min_pool_liquidity_usd", json!(100)),
    ] {
        ctx.settings
            .set(n, v.clone(), Some(x), "тест")
            .await
            .unwrap();
    }
    let p = Parts::new(ctx.clone());
    let (e, _, _) = aave_v3(&rec());
    p.registry.put(e.clone());
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let pools = p.pools(tx);
    let r = e.pool.reference.clone();
    let q = p.quoting();
    let frozen = freeze(&pools, [&r].into_iter());
    assert_eq!(frozen, vec![r.clone()]);
    let why = usable(&q, &p.registry.get(&r).unwrap(), Side::Sell, 18).unwrap_err();
    assert!(why.contains("недостоверны"), "{why}");
    thaw(&pools, &frozen, &[]);
    assert!(
        p.registry.get(&r).unwrap().valid,
        "после чтения — снова достоверен"
    );
    // Не прочитавшийся — остаётся недостоверным.
    let frozen = freeze(&pools, [&r].into_iter());
    thaw(&pools, &frozen, std::slice::from_ref(&r));
    assert!(!p.registry.get(&r).unwrap().valid);
}
