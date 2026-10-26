//! Файл: tests/bsc_dex_depth_sides.rs
//! Что делает: решение 73 (решение Тиграна) — стороны пула независимо, на живом пуле AAVE Uniswap V3 0x8e3ecc0b…
//! (запись 2026-10-25; глубина ±2% без комиссии: покупка ≈ 797,1, продажа ≈ 800,6 USDT):
//! - пул, глубокий только на продажу (порог 799), с прошедшей сверкой продажи, котируется на продажу и не котируется
//!   на покупку (quote/select.rs usable);
//! - падение глубины покупки (порог 700 → 799) не убирает продажу: её сверка не сбрасывается (check/quoter_sides.rs —
//!   мелкая сторона без итога, другая не меняется), продажа котируется, покупка — нет;
//! - сторона без своей сверки не котируется, даже если другая сверена.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/native_common, tests/depth_common, arb::venues::bsc_dex::{parts, quote::select,
//! check::quoter_sides}.

mod depth_common;
mod native_common;

use serde_json::json;

use arb::core::types::{Asset, Side};
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::quote::select::usable;

use depth_common::{aave_v3, rec};

async fn set(ctx: &Ctx, name: &str, scope: Scope, v: serde_json::Value) {
    ctx.settings
        .set(name, scope, Some(v), "тест")
        .await
        .unwrap();
}

#[tokio::test]
async fn sides_are_independent() {
    let ctx = native_common::ctx("depth-sides").await;
    let v = Scope::Venue("bsc_dex".into());
    for (n, x) in [
        ("venue.quote_asset", json!("USDT")),
        ("dex.depth_band_pct", json!(2)),
        ("dex.depth_hysteresis_pct", json!(0)),
        ("dex.min_pool_liquidity_usd", json!(799)),
    ] {
        set(&ctx, n, v.clone(), x).await;
    }
    let u3 = Scope::Dex("uniswap_v3".into());
    set(&ctx, "dex.pool_enabled", u3.clone(), json!(true)).await;
    // Остальные адреса строки DEX, без которых она «не подключена» (pools/families.rs required) — условные: сеть в
    // тесте не нужна (как в tests/bsc_dex_via.rs).
    for n in ["dex.factory", "dex.tick_lens", "dex.quoter"] {
        let a = json!("0x4000000000000000000000000000000000000004");
        set(&ctx, n, u3.clone(), a).await;
    }
    let p = Parts::new(ctx.clone());
    p.tokens.set(&[Asset {
        ticker: "USDT".into(),
        bsc_contract: Some(alloy_primitives::address!(
            "0x55d398326f99059ff775485246999027b3197955"
        )),
    }]);
    let (e, _, _) = aave_v3(&rec());
    p.registry.put(e.clone());
    let r = e.pool.reference.clone();
    // Глубока только продажа — цикл сверил бы только её.
    p.checks.update_pool(&r, |f| {
        f.gas_buy = Some(150_000);
        f.gas_sell = Some(150_000);
        f.quoter.set(Side::Sell, Some(Ok(())));
    });
    let q = p.quoting();
    let s = usable(&q, &e, Side::Sell, 18);
    assert!(s.is_ok(), "продажа котируется: {:?}", s.err());
    let why = usable(&q, &e, Side::Buy, 18).unwrap_err();
    assert!(why.contains("глубина покупки"), "{why}");
    // Обе глубоки и сверены.
    set(&ctx, "dex.min_pool_liquidity_usd", v.clone(), json!(700)).await;
    p.checks
        .update_pool(&r, |f| f.quoter.set(Side::Buy, Some(Ok(()))));
    assert!(usable(&q, &e, Side::Buy, 18).is_ok());
    assert!(usable(&q, &e, Side::Sell, 18).is_ok());
    // Глубина покупки упала: цикл снимает итог только у неё — продажа остаётся в котировках.
    set(&ctx, "dex.min_pool_liquidity_usd", v.clone(), json!(799)).await;
    p.checks.update_pool(&r, |f| f.quoter.set(Side::Buy, None));
    assert_eq!(p.checks.pool(&r).quoter.sell, Some(Ok(())));
    assert!(usable(&q, &e, Side::Sell, 18).is_ok(), "продажа не убрана");
    assert!(usable(&q, &e, Side::Buy, 18).is_err());
    // Покупка снова глубокая, но своей сверки ещё нет — не котируется, хотя продажа сверена.
    set(&ctx, "dex.min_pool_liquidity_usd", v, json!(700)).await;
    let why = usable(&q, &e, Side::Buy, 18).unwrap_err();
    assert!(why.contains("не сверена"), "{why}");
}
