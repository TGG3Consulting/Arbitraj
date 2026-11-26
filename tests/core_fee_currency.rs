//! Одинаковые правила для запроса курса и оценки строки затрат.
mod usdt_return_common;

use arb::core::convert::{rates, to_usdt};
use arb::core::types::{Cost, CostKind};
use rust_decimal::Decimal;

#[tokio::test]
async fn zero_fee_needs_no_market_but_nonzero_fee_requires_its_currency() {
    let (core, _) = usdt_return_common::start(&usdt_return_common::dir("fee_currency"), None).await;
    let deps = &core.inner.deps;
    let coin_price = Decimal::new(5, 2);
    let fee = |amount| Cost::new(CostKind::WithdrawFee, amount, "TSTBSC", false);
    let zero = fee(Decimal::ZERO);
    let actual = Cost::new(CostKind::TransferShortfall, Decimal::TEN, "TST", false);
    let quote = vec!["USDT".to_owned()];
    let current = rates(
        deps,
        &[],
        quote.clone(),
        "TST",
        coin_price,
        &[zero.clone(), actual.clone()],
    )
    .await
    .unwrap();
    assert_eq!(to_usdt(&zero, &current).unwrap(), Decimal::ZERO);
    assert_eq!(to_usdt(&actual, &current).unwrap(), Decimal::new(5, 1));

    let charged = fee(Decimal::new(1, 8));
    assert!(to_usdt(&charged, &current).is_err());
    let error = rates(deps, &[], quote, "TST", coin_price, &[charged])
        .await
        .err()
        .unwrap();
    assert!(error.contains("WithdrawFee 0.00000001"));
    assert!(error.contains("нет живой цены TSTBSC"));
}
