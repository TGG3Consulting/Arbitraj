use super::*;

fn prices(here: Result<Decimal, String>, targets: &[(&str, i64)]) -> Comparison {
    Comparison {
        here,
        targets: targets
            .iter()
            .map(|(v, n)| (VenueId((*v).into()), Ok((*n).into())))
            .collect(),
        skipped: Vec::new(),
        prior_cash: Decimal::ZERO,
        here_rest_error: None,
        wallets: HashMap::new(),
    }
}

#[test]
fn compares_net_proceeds_and_keeps_ties_at_home() {
    let q = prices(Ok(80.into()), &[("a", 80), ("b", 100)]);
    assert_eq!(q.better_target().map(|(v, _)| v.0.as_str()), Some("b"));
    assert_eq!(q.difference(100.into()), Some((20.into(), Some(25.into()))));
    assert!(
        prices(Ok(100.into()), &[("b", 100)])
            .better_target()
            .is_none()
    );
}

#[test]
fn combines_sale_with_prior_fills_only_on_its_own_venue() {
    let a = VenueId("a".into());
    let b = VenueId("b".into());
    let past = [(a.clone(), 7.into())];
    assert_eq!(
        return_amounts(&past, &a, 9.into()),
        vec![(a.clone(), 16.into())]
    );
    assert_eq!(
        return_amounts(&past, &b, 9.into()),
        vec![(a, 7.into()), (b, 9.into())]
    );
}

#[test]
fn no_invented_baseline_or_percentage() {
    let q = prices(Err("нет котировки".into()), &[("b", 100)]);
    assert!(q.better_target().is_some());
    assert_eq!(q.difference(100.into()), None);
    assert_eq!(
        prices(Ok(Decimal::ZERO), &[]).difference(10.into()),
        Some((10.into(), None))
    );
}

#[tokio::test]
async fn tradoor_left_group_values_the_actual_separate_return_before_choosing() {
    use crate::core::types::{Asset, CoinNetworkInfo, Cost, CostKind, Quote};
    use crate::core::{
        coin_group_left, deal::Step, deal_group::DealGroup, engine::wr, relay_here, sell_turn_fresh,
    };
    use crate::gateway::contract::VenueKind;
    use crate::settings::value::Scope;
    use serde_json::{Value, json};

    let folder = std::env::temp_dir().join(format!(
        "arb-tradoor-return-{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap()
    ));
    std::fs::create_dir_all(&folder).unwrap();
    let core = crate::core::test_core::open(&folder).await;
    let source: Value = serde_json::from_str(include_str!(
        "../../../tests/data/tradoor_return_2026_11_22/amounts.json"
    ))
    .unwrap();
    let num = |k: &str| source[k].as_str().unwrap().parse::<Decimal>().unwrap();
    let mut d = Deal::new(
        source["id"].as_str().unwrap().into(),
        serde_json::from_value(source["direction"].clone()).unwrap(),
        Value::Null,
        Decimal::ZERO,
    );
    d.dex_account = Some(serde_json::from_value(source["dex_account"].clone()).unwrap());
    d.at = d.direction.sell_on.clone();
    d.invested = num("invested");
    d.spent = num("spent_at_close") - num("fee");
    d.qty = num("qty");
    d.group = Some(Box::new(DealGroup::new(&d.id)));
    d.step = Step::Selling;
    let venue = d.at.clone();
    let home = d.direction.buy_on.clone();
    let address = "0x55d398326f99059ff775485246999027b3197955"
        .parse()
        .unwrap();
    let usdt = Asset {
        ticker: "USDT".into(),
        contract: Some(address),
        chain: None,
    };
    for (name, scope, val) in [
        ("usdt_return.mode", Scope::Global, json!("per_trade")),
        (
            "venue.quote_asset",
            Scope::Venue(venue.0.clone()),
            json!("USDT"),
        ),
        (
            "venue.networks_stale_s",
            Scope::Venue(venue.0.clone()),
            json!(3600),
        ),
    ] {
        core.inner
            .deps
            .settings
            .set(name, scope, Some(val), "test")
            .await
            .unwrap();
    }
    let q = Quote {
        venue: venue.clone(),
        asset: d.direction.asset.clone(),
        side: Side::Sell,
        qty: d.qty,
        fillable_qty: d.qty,
        avg_price: num("avg"),
        costs: vec![Cost {
            kind: CostKind::TradeFee,
            asset: "USDT".into(),
            amount: num("fee"),
            in_price: false,
            step: None,
            paid_in: None,
        }],
        route: None,
        signal_route: None,
        pick: None,
        at: chrono::Utc::now(),
    };
    let raw = num("gross") - num("fee");
    let quote = || Ok((raw, q.clone(), None));
    assert_eq!(
        sell_turn_fresh::settled(&core, &d, quote())
            .await
            .unwrap()
            .0,
        raw
    );
    assert_eq!(d.loss_pct_if(raw).round_dp(2), Decimal::new(121, 2));
    let mut stuck = d.clone();
    stuck.step = Step::Stuck;
    crate::core::ledger::save(&core, &stuck).await;
    // The in-flight task still holds the pre-save copy. A second save must keep the first timestamp.
    assert!(!d.group.as_ref().unwrap().left);
    let first = core.deals().into_iter().find(|x| x.id == d.id).unwrap();
    crate::core::ledger::save(&core, &d).await;
    assert_eq!(
        core.deals()
            .into_iter()
            .find(|x| x.id == d.id)
            .unwrap()
            .timeline,
        first.timeline
    );
    // Install the frozen directory after persisting the group transition. Registry
    // watchers may rebuild on settings changes while an asynchronous ledger save yields.
    *wr(&core.inner.kinds) = vec![
        (home.clone(), VenueKind::Dex),
        (venue.clone(), VenueKind::Cex),
    ];
    *wr(&core.inner.usdt) = Ok(address);
    wr(&core.inner.networks).insert(
        venue.clone(),
        std::sync::Arc::new(vec![CoinNetworkInfo {
            venue: venue.clone(),
            asset: usdt,
            network: "BSC".into(),
            deposit_enabled: true,
            withdraw_enabled: true,
            withdraw_fee: Some(num("withdraw_fee")),
            withdraw_fee_pct: None,
            use_net_withdrawal_budget: false,
            withdraw_min: Some(Decimal::ONE),
            withdraw_max: None,
            deposit_min: None,
            deposit_confirmations: None,
            withdraw_precision: Some(8),
            withdraw_step: None,
            updated_at: chrono::Utc::now(),
        }]),
    );
    let value = sell_turn_fresh::settled(&core, &d, quote())
        .await
        .unwrap()
        .0;
    assert_eq!(value.round_dp(8), num("credited"));
    assert_eq!(num("sent") - num("credited"), num("withdraw_fee"));
    assert_eq!(d.result_if(value).round_dp(6), num("result").round_dp(6));
    assert_eq!(d.loss_pct_if(value).round_dp(2), Decimal::new(439, 2));
    assert!(matches!(
        relay_here::decide(&d, "fixture", Ok(20.into()), Ok(value)),
        relay_here::Here::Market(_)
    ));
    assert!(matches!(
        relay_here::decide(&d, "fixture", Ok(3.into()), Ok(value)),
        relay_here::Here::Alert { .. }
    ));
    assert!(matches!(
        relay_here::decide(&d, "fixture", Ok(3.into()), Ok(raw)),
        relay_here::Here::Market(_)
    ));
    // At the origin there is no return on the remaining tokens; an earlier partial sale
    // elsewhere still owes its single withdrawal. Quotes include only its return cost.
    assert_eq!(
        settlement_cost(&core, &d, &home, None, &[], raw)
            .await
            .unwrap(),
        Decimal::ZERO
    );
    let earlier = vec![(venue.clone(), num("sent"))];
    assert_eq!(
        settlement_cost(&core, &d, &home, None, &earlier, 10.into())
            .await
            .unwrap(),
        num("withdraw_fee")
    );
    assert_eq!(
        settlement_cost(&core, &d, &venue, None, &earlier, 10.into())
            .await
            .unwrap(),
        num("withdraw_fee")
    );
    // The first automatic individual withdrawal cannot use the manual retry's splitter.
    let mut networks = wr(&core.inner.networks)
        .get(&venue)
        .unwrap()
        .as_ref()
        .clone();
    networks[0].withdraw_max = Some(Decimal::from(6));
    wr(&core.inner.networks).insert(venue.clone(), std::sync::Arc::new(networks));
    assert!(
        settlement_cost(&core, &d, &venue, None, &earlier, 10.into())
            .await
            .is_err()
    );
    wr(&core.inner.networks).clear();
    assert!(sell_turn_fresh::settled(&core, &d, quote()).await.is_err());
    // A separate-mode deal explicitly holding proceeds uses the quoted sale amount.
    core.inner
        .deps
        .settings
        .set(
            "usdt_return.mode",
            Scope::Global,
            Some(json!("separate")),
            "test",
        )
        .await
        .unwrap();
    coin_group_left::sync(&core, &mut d);
    assert!(!needs_individual_return(&core, &d));
    assert_eq!(
        sell_turn_fresh::settled(&core, &d, quote())
            .await
            .unwrap()
            .0,
        raw
    );
}
