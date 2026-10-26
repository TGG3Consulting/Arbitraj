//! Файл: tests/bsc_dex_depth_sides.rs
//! Что делает: решение 73 (решение Тиграна) — стороны пула независимо, на живом пуле AAVE Uniswap V3 0x8e3ecc0b…
//! (запись 2026-10-25; глубина ±2% без комиссии: покупка ≈ 797,1, продажа ≈ 800,6 USDT):
//! - пул, глубокий только на продажу (порог 799), с прошедшей сверкой продажи, котируется на продажу и не котируется
//!   на покупку (quote/select.rs usable);
//! - падение глубины покупки (порог 700 → 799) не убирает продажу: её сверка не сбрасывается (check/quoter_sides.rs —
//!   мелкая сторона без итога, другая не меняется), продажа котируется, покупка — нет;
//! - сторона без своей сверки не котируется до сверки, даже если другая сверена; стала глубокой — «пул готов», цикл
//!   (check/quoter_cycle.rs one, отбор по check/quoter_side.rs side_ready) сверяет её одним проходом, не дожидаясь
//!   dex.quoter_check_s.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/native_common, tests/depth_common, arb::venues::bsc_dex::{parts, quote::select,
//! check::quoter_sides}.

mod depth_common;
mod native_common;

use serde_json::json;

use arb::core::types::{Asset, Side};
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::check::fresh::Fresh;
use arb::venues::bsc_dex::check::quoter_cycle;
use arb::venues::bsc_dex::check::quoter_side::side_ready;
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
    let usdt = alloy_primitives::address!("0x55d398326f99059ff775485246999027b3197955");
    p.tokens.set(&[Asset {
        ticker: "USDT".into(),
        bsc_contract: Some(usdt),
    }]);
    // decimals() USDT в BSC — 18 (в записи глубины балансы и глубины USDT сходятся с замером только при 18).
    p.tokens.remember_decimals(usdt, 18);
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
    // Покупка снова глубокая, но своей сверки ещё нет — не котируется до сверки, хотя продажа сверена; сверка — по
    // сигналу «пул готов», а не через полный проход dex.quoter_check_s (ревью #63).
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let pools = p.pools(tx);
    let mut fresh = Fresh::default();
    fresh.mark(&r);
    assert!(!side_ready(&pools, &r), "покупка мелкая — сверять нечего");
    assert!(fresh.due(&pools, |x| side_ready(&pools, x)).is_empty());
    set(&ctx, "dex.min_pool_liquidity_usd", v, json!(700)).await;
    let w = pools.signals.ready.notified();
    tokio::pin!(w);
    w.as_mut().enable();
    let why = usable(&q, &e, Side::Buy, 18).unwrap_err();
    assert!(why.contains("не сверена"), "{why}");
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(200), w)
            .await
            .is_ok(),
        "глубина выросла — «пул готов»"
    );
    assert!(side_ready(&pools, &r), "сторона без итога стала глубокой");
    assert_eq!(
        fresh.due(&pools, |x| side_ready(&pools, x)),
        vec![r.clone()]
    );
    // Один проход цикла: сторона покупки сверена (квотера в тесте нет — итог «не задана настройка узла», но он есть,
    // а не «ещё не сверена»; ответ квотера на записи — отдельной записью, см. tests/README.md).
    assert!(quoter_cycle::one(&pools, &r, &mut None).await);
    let buy = p.checks.pool(&r).quoter.buy;
    assert!(buy.is_some(), "сторона покупки сверена одним проходом");
    let why = usable(&q, &e, Side::Buy, 18).unwrap_err();
    assert!(!why.contains("не сверена"), "{why}");
    assert!(
        !side_ready(&pools, &r),
        "итог есть — повторять по сигналу нечего"
    );
}
