//! Файл: tests/bsc_dex_config_wake2.rs
//! Что делает: решение 75, дополнения ревью #65:
//! - смена порога глубины на лету (живой бот 26.10: dex.min_pool_liquidity_usd 2000 → 200, налоги висели «глубина
//!   ниже порога»): пересчёт вердикта по прочитанной карте без событий пула (pools/depth.rs touch — как делает
//!   pools/depth_sync.rs для каждого пула пачки) → «пул готов» → монета, ждущая глубокого пула (Why::Shallow), идёт на
//!   проверку сразу (живой пул AAVE V3 0x8e3ecc0b…, запись 2026-10-25);
//! - ошибки ленты называют узел — только хост, без пути и параметров (запись глубины 25.10: ответ «missing trie
//!   node» узла bsc-dataseed);
//! - «лента догнала сеть»: в тексте — окно bsc.receipts_ahead_blocks, а не число, выдаваемое за фактическое
//!   отставание;
//! - общий механизм ожидания настройки (settings/wait.rs, им пользуются и BingX: ставки комиссии, справочник пар):
//!   одна запись «ждёт настройку» и одна «снова работает», чужие изменения настроек не будят запись повторно.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/native_common, tests/depth_common, arb::venues::bsc_dex::{parts, pools::depth,
//! check::tax_wait, node::{rpc_race, rpc_capped, receipts_lag}}, arb::settings::wait.

mod depth_common;
mod native_common;

use std::time::Duration;

use serde_json::json;

use arb::core::types::Asset;
use arb::settings::read::At;
use arb::settings::scope::Scope;
use arb::settings::wait::{Note, until_ok};
use arb::venues::bsc_dex::check::tax_pass::Why;
use arb::venues::bsc_dex::check::tax_wait::due;
use arb::venues::bsc_dex::node::receipts_lag::Lag;
use arb::venues::bsc_dex::node::rpc_capped::split_from;
use arb::venues::bsc_dex::node::rpc_race::host_source;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::pools::depth;

use depth_common::{aave_v3, rec};

#[tokio::test]
async fn threshold_change_recomputes_and_wakes_shallow() {
    let ctx = native_common::ctx("config-wake-depth").await;
    let v = Scope::Venue("bsc_dex".into());
    for (n, x) in [
        ("venue.quote_asset", json!("USDT")),
        ("dex.depth_band_pct", json!(2)),
        ("dex.depth_hysteresis_pct", json!(10)),
        ("dex.min_pool_liquidity_usd", json!(2000)),
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
    p.tokens.remember_decimals(usdt, 18);
    let (e, _, _) = aave_v3(&rec());
    p.registry.put(e.clone());
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let pools = p.pools(tx);
    let r = e.pool.reference.clone();
    // Порог 2000: мелкий (≈ 800 USDT в ±2%) — монета ждёт глубокого пула.
    depth::touch(&pools, &r);
    let d = p.registry.depth().last(&r).unwrap();
    assert!(!d.deep_buy && !d.deep_sell);
    let waiting = vec![(e.coin, "AAVE".to_string(), Why::Shallow)];
    let deep_now = |t: &str| arb::venues::bsc_dex::check::tax_deep::has_deep(&pools, t);
    assert!(due(&waiting, true, |_| 0, deep_now).is_empty());
    // Порог снизили до 200 — событий у пула нет; пачка пересчитывает вердикт по карте (как depth_sync.rs).
    ctx.settings
        .set("dex.min_pool_liquidity_usd", v, Some(json!(200)), "Тигран")
        .await
        .unwrap();
    let w = pools.signals.ready.notified();
    tokio::pin!(w);
    w.as_mut().enable();
    depth::touch(&pools, &r);
    assert!(
        tokio::time::timeout(Duration::from_millis(200), w)
            .await
            .is_ok(),
        "стало глубоким — «пул готов»"
    );
    let d = p.registry.depth().last(&r).unwrap();
    assert!(d.deep_buy && d.deep_sell);
    assert_eq!(
        due(&waiting, true, |_| 0, deep_now),
        vec![(e.coin, "AAVE".to_string())]
    );
}

#[test]
fn node_errors_name_the_host_only() {
    assert_eq!(
        host_source("https://bsc-dataseed.bnbchain.org/abc?key=secret"),
        "узел BSC (HTTP) bsc-dataseed.bnbchain.org"
    );
    // Ответ узла из записи глубины 2026-10-25 (запись координатора: все 1004 ошибки — этот ответ).
    let body =
        r#"[{"jsonrpc":"2.0","id":0,"error":{"code":-32000,"message":"missing trie node"}}]"#;
    let src = host_source("https://bsc-dataseed.bnbchain.org");
    let list = split_from(body, 1, &src).unwrap();
    let e = list[0].as_ref().unwrap_err();
    assert!(
        e.message.contains("bsc-dataseed.bnbchain.org"),
        "{}",
        e.message
    );
    assert!(!e.message.contains("secret"));
}

#[tokio::test]
async fn caught_up_names_window_not_lag() {
    let ctx = native_common::ctx("config-wake-lag").await;
    let mut lag = Lag::default();
    lag.report(&ctx, 20, 6).await;
    lag.report(&ctx, 6, 6).await;
    let all = ctx.journal.list(20, None).await.unwrap();
    let m = all
        .iter()
        .find(|e| e.message.contains("догнала"))
        .map(|e| e.message.clone())
        .unwrap();
    assert!(m.contains("≤ окна bsc.receipts_ahead_blocks (6)"), "{m}");
    assert!(!m.contains("отставание 6 блоков"), "{m}");
}

#[tokio::test]
async fn shared_wait_logs_once_and_resumes() {
    let ctx = native_common::ctx("config-wake-shared").await;
    let c2 = ctx.clone();
    let wait = tokio::spawn(async move {
        let note = Note {
            journal: &c2.journal,
            kind: "venue.fees",
            venue: Some("bingx"),
        };
        let at = At {
            venue: Some("bingx"),
            coin: None,
        };
        let read = || c2.settings.integer("venue.fee_cache_s", at);
        until_ok(&c2.settings, note, "Обновление ставок комиссии", read).await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    // Чужая настройка — запись «ждёт» не повторяется.
    ctx.settings
        .set(
            "tax.recheck_interval_s",
            Scope::Global,
            Some(json!(3600)),
            "Тигран",
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!wait.is_finished());
    ctx.settings
        .set(
            "venue.fee_cache_s",
            Scope::Venue("bingx".into()),
            Some(json!(600)),
            "Тигран",
        )
        .await
        .unwrap();
    let got = tokio::time::timeout(Duration::from_secs(2), wait)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got, Some(600));
    let all = ctx.journal.list(50, None).await.unwrap();
    let waits = all
        .iter()
        .filter(|e| e.message.contains("ждёт настройку"))
        .count();
    let back = all
        .iter()
        .filter(|e| e.message.contains("снова работает"))
        .count();
    assert_eq!((waits, back), (1, 1));
}
