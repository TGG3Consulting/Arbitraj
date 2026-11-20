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
//! Время (T472, нестабильность): тест не опирается на паузы угаданной длины — ждёт событие (запись в журнале) опросом
//! с пределом 10 с; конец окна второй проверки считается от момента, когда отложенная запись уже видна в журнале
//! (окно пула отсчитывается от std::time::Instant записи — depth_log.rs; tokio::time::pause его не двигает, поэтому
//! управляемое время невозможно без правки кода). Окно — 2 с: остаётся единственное допущение — между сменами внутри
//! окна проходит меньше 2 с (несколько записей настроек; раньше при окне 1 с запас был ≈ 0,3–0,6 с).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/native_common, tests/depth_common, arb::venues::bsc_dex::{parts, pools::depth_book,
//! quote::select}.

mod depth_common;
mod native_common;

use std::sync::Arc;
use std::time::{Duration, Instant};

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

/// Окно журнала dex.depth_log_min_s в тесте, с.
const WIN: u64 = 2;

/// Ждёт, пока записей о смене станет не меньше `n` (опрос журнала каждые 20 мс, предел 10 с — ожидание события, а не
/// пауза угаданной длины). Возвращает записи (новые первыми); не дождался — сколько есть (проверка упадёт).
async fn said_n(ctx: &Ctx, n: usize) -> Vec<String> {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        let s = said(ctx).await;
        if s.len() >= n || Instant::now() > end {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
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
        ("dex.depth_log_min_s", json!(WIN)),
        ("dex.min_pool_liquidity_usd", json!(700)),
    ] {
        set(&ctx, n, v).await;
    }
    let p = Parts::new(ctx.clone());
    p.tokens.set(&[Asset {
        chain: None,
        ticker: "USDT".into(),
        contract: Some(usdt),
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
    let s = said_n(&ctx, 1).await;
    assert_eq!(s.len(), 1);
    assert!(s[0].contains("покупка — мелкий"));
    // Порог 700: снова глубокий — сигнал «пул готов» ровно один раз, хотя первым считал не отбор котировки, а «экран»
    // (Book::depth, как depth_facts.rs); запись — отложена до конца окна в WIN с.
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
    let s = said_n(&ctx, 2).await;
    // Отложенная запись уже в журнале — её окно началось не позже этого мгновения.
    let w = Instant::now();
    assert_eq!(s.len(), 2, "отложенная смена записана в конце окна");
    assert!(s[0].contains("покупка — глубокий"), "{}", s[0]);
    // Мелкий и снова глубокий внутри окна — в конце окна состояние как записанное: записи нет.
    set(&ctx, "dex.min_pool_liquidity_usd", json!(900)).await;
    assert!(side(Need::Both).unwrap().is_some());
    set(&ctx, "dex.min_pool_liquidity_usd", json!(700)).await;
    assert_eq!(side(Need::Both), Ok(None));
    // Отложенная проверка срабатывает не позже w + WIN; запас 0,5 с — на её задачу и запись (если бы она была).
    let end = w + Duration::from_secs(WIN) + Duration::from_millis(500);
    tokio::time::sleep_until(end.into()).await;
    assert_eq!(
        said(&ctx).await.len(),
        2,
        "вернулось к записанному — без записи"
    );
}
