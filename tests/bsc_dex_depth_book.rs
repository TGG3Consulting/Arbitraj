//! Файл: tests/bsc_dex_depth_book.rs
//! Что делает: решение 73 — глубина пула в памяти реестра (pools/depth_book.rs, depth_log.rs) на живом пуле AAVE
//! Uniswap V3 0x8e3ecc0b… (запись 2026-10-25; глубина ±2%: покупка ≈ 799,5, продажа ≈ 800,6 USDT):
//! - гистерезис через кэш с прошлым вердиктом: порог 850 при пороге «вниз» 765 (гистерезис 10%) — пул остаётся
//!   глубоким; порог 900 (вниз 810) — мелкий;
//! - журнал смен: первая смена — сразу; смена внутри окна dex.depth_log_min_s не теряется — пишется в конце окна
//!   последним состоянием; вернулось к записанному — записи нет;
//! - сигнал «пул готов» при росте — ровно один раз, кто бы первым ни посчитал глубину на новом состоянии (как
//!   экран «Монеты» — depth_facts.rs зовёт тот же Book::depth); повторный расчёт без смены — без сигнала.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/native_common, tests/depth_common, arb::venues::bsc_dex::{parts, pools::depth_book,
//! quote::select}.

mod depth_common;
mod native_common;

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use arb::core::types::{Asset, Side};
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::quote::select::{Need, deep_side};

use depth_common::{aave_v3, rec};

async fn set(ctx: &Ctx, name: &str, v: serde_json::Value) {
    let s = Scope::Venue("bsc_dex".into());
    ctx.settings.set(name, s, Some(v), "тест").await.unwrap();
}

/// Записи журнала о смене глубины этого пула (новые первыми).
async fn said(ctx: &Ctx) -> Vec<String> {
    let all = ctx.journal.list(200, None).await.unwrap();
    all.into_iter()
        .filter(|e| e.kind == "dex.pool" && e.message.contains("в полосе ±"))
        .map(|e| e.message)
        .collect()
}

/// Сработает ли сигнал за `ms` мс (ожидание взведено до действия `act`).
async fn fires(n: &Arc<tokio::sync::Notify>, ms: u64, act: impl FnOnce()) -> bool {
    let w = n.notified();
    tokio::pin!(w);
    w.as_mut().enable();
    act();
    tokio::time::timeout(Duration::from_millis(ms), w)
        .await
        .is_ok()
}

#[tokio::test]
async fn hysteresis_journal_and_rise_signal() {
    let ctx = native_common::ctx("depth-book").await;
    let usdt = alloy_primitives::address!("0x55d398326f99059ff775485246999027b3197955");
    for (n, v) in [
        ("venue.quote_asset", json!("USDT")),
        ("dex.depth_band_pct", json!(2)),
        ("dex.depth_hysteresis_pct", json!(10)),
        ("dex.depth_log_min_s", json!(1)),
        ("dex.min_pool_liquidity_usd", json!(700)),
    ] {
        set(&ctx, n, v).await;
    }
    let p = Parts::new(ctx.clone());
    p.tokens.set(&[Asset {
        ticker: "USDT".into(),
        bsc_contract: Some(usdt),
    }]);
    let ready = Arc::new(tokio::sync::Notify::new());
    p.registry.depth().on_rise(ready.clone());
    let (e, _, _) = aave_v3(&rec());
    p.registry.put(e.clone());
    let (reg, tok) = (&p.registry, &p.tokens);
    let side = |need| deep_side(&ctx, reg, tok, &e, 18, need);
    // Первый расчёт — точка отсчёта: глубокий, без записи и без сигнала.
    assert!(!fires(&ready, 50, || assert_eq!(side(Need::Both), Ok(None))).await);
    // Порог 850: глубина ≈ 800 ниже, но выше 850 × 0,9 — по прошлому вердикту остаётся глубоким.
    set(&ctx, "dex.min_pool_liquidity_usd", json!(850)).await;
    assert_eq!(side(Need::Side(Side::Buy)), Ok(None), "гистерезис держит");
    // Порог 900: вниз — ниже 810 — мелкий; первая смена — в журнал сразу.
    set(&ctx, "dex.min_pool_liquidity_usd", json!(900)).await;
    assert!(side(Need::Both).unwrap().is_some());
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(said(&ctx).await.len(), 1);
    assert!(said(&ctx).await[0].contains("покупка — мелкий"));
    // Порог 700: снова глубокий — сигнал «пул готов» ровно один раз, хотя первым считал не отбор котировки, а «экран»
    // (Book::depth, как depth_facts.rs); запись — отложена до конца окна в 1 с.
    set(&ctx, "dex.min_pool_liquidity_usd", json!(700)).await;
    let first = || {
        let d = p.registry.depth().depth(&ctx, reg, tok, &e, 18).unwrap();
        assert!(d.deep_buy && d.deep_sell);
    };
    assert!(fires(&ready, 200, first).await, "сигнал при росте");
    assert!(
        !fires(&ready, 100, || assert_eq!(side(Need::Both), Ok(None))).await,
        "второй раз — нет"
    );
    assert_eq!(said(&ctx).await.len(), 1, "внутри окна — не сразу");
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let s = said(&ctx).await;
    assert_eq!(s.len(), 2, "отложенная смена записана в конце окна");
    assert!(s[0].contains("покупка — глубокий"), "{}", s[0]);
    // Мелкий и снова глубокий внутри окна — в конце окна состояние как записанное: записи нет.
    set(&ctx, "dex.min_pool_liquidity_usd", json!(900)).await;
    assert!(side(Need::Both).unwrap().is_some());
    set(&ctx, "dex.min_pool_liquidity_usd", json!(700)).await;
    assert_eq!(side(Need::Both), Ok(None));
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(
        said(&ctx).await.len(),
        2,
        "вернулось к записанному — без записи"
    );
}
