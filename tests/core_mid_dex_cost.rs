//! Файл: tests/core_mid_dex_cost.rs
//! Что делает: решение 276 (дополнение — ревью, п. 5) — главное денежное утверждение решения 276 в настоящем расчёте
//! ядра costs::evaluate_with: затрата шага в промежуточной монете, которой не знает биржа (подпись «FIST», курса у ядра
//! нет), с in_price = true — расчёт Ok, прибыль и сумма затрат те же, что без этой строки; строка в показе с
//! usdt = None. Та же строка с in_price = false — отказ (курса FIST нет): строка действительно доходит до расчёта.
//! Данные — записанный план CAKE (tests/report_plan: стакан BingX CAKE-USDT 23.10, пул CAKE/USDT V2); строка FIST
//! добавляется в затраты котировки продажи DEX (как quote/steps.rs path_costs кладёт PoolFee шага в промежуточной).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{costs, size}, tests/report_plan, tests/size_common.
//! Сумма строки FIST (1) — вход теста: строка «в цене» на деньги не влияет при любой сумме.

mod report_plan;
mod size_common;

use arb::core::size::Fail;
use arb::core::types::CostKind;
use serde_json::{Value, json};

/// Правка плана: строка PoolFee в «FIST» в затратах котировки продажи DEX. Принимает: in_price строки.
fn with_fist(in_price: bool) -> impl FnOnce(&mut Value) {
    move |pj: &mut Value| {
        pj["sell_quote"]["costs"].as_array_mut().unwrap().push(json!({
            "kind": "PoolFee", "amount": "1", "asset": "FIST", "in_price": in_price,
            "step": {"index": 1, "from": "CAKE", "to": "FIST", "from_token": null, "to_token": null}
        }));
    }
}

#[tokio::test]
async fn in_price_cost_in_unknown_ticker_keeps_profit() {
    let base = report_plan::plan_edit(|_| {}).await.unwrap();
    let got = report_plan::plan_edit(with_fist(true))
        .await
        .expect("затрата «в цене» без курса не мешает расчёту");
    assert_eq!(got.profit_usd, base.profit_usd);
    assert_eq!(got.costs_usdt, base.costs_usdt);
    assert_eq!(got.proceeds, base.proceeds);
    assert_eq!(got.invested, base.invested);
    let fist: Vec<_> = got.costs.iter().filter(|c| c.asset == "FIST").collect();
    assert_eq!(fist.len(), 1);
    assert_eq!(fist[0].kind, CostKind::PoolFee);
    assert!(fist[0].in_price);
    assert_eq!(fist[0].usdt, None, "курса FIST нет — только показ");
    assert_eq!(got.costs.len(), base.costs.len() + 1);
    // Та же строка, но вычитаемая — курса нет, расчёт отказывает.
    let err = report_plan::plan_edit(with_fist(false)).await.unwrap_err();
    assert!(matches!(err, Fail::TooLarge(_)), "{err:?}");
    assert!(err.text().contains("FIST"), "{err:?}");
}
