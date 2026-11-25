use super::*;

fn prices(here: Result<Decimal, String>, targets: &[(&str, i64)]) -> Comparison {
    Comparison {
        here,
        targets: targets
            .iter()
            .map(|(v, n)| (VenueId((*v).into()), Ok((*n).into())))
            .collect(),
        skipped: Vec::new(),
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
