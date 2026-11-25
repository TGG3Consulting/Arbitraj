

#[tokio::test]
async fn stay_recorded_transfers_do_not_need_kucoin_status_with_chain_credit() {
    const TOKEN: &str = "0x30c0ef9361645ec56a7e640cd05ce6abdfce8422";
    const CASES: [(&str, &str, &str, u64); 2] = [
        (
            "20261107083933889STAY",
            "0xa5cd3b84e39daa725aa53b761f10b3791f169437cc028621d37d216e0f128916",
            "409321cf2eea544c40000",
            132170216,
        ),
        (
            "20261107095539774STAY",
            "0x4a198805e2e8bca7e0d06b87f641a9d5125a4641cf0ab1f129e83037d459fb3c",
            "51b266191c2b441040000",
            132180354,
        ),
    ];
    let (core, settings) = start("stay").await;
    let venue = Scope::Venue("bsc_dex".into());
    settings
        .set(
            "transfer.credit_amount_tolerance_pct",
            venue.clone(),
            Some(json!("0")),
            "STAY replay",
        )
        .await
        .unwrap();
    // Generic event credit was unset in the historical profile. The chain
    // credit switch must work independently of that flag.
    for enabled in [false, true] {
        settings
            .set(
                "transfer.credit_by_chain",
                venue.clone(),
                enabled.then(|| json!(true)),
                "STAY replay",
            )
            .await
            .unwrap();
        for (deal, hash, raw, block) in CASES {
            let units: u128 = U256::from_str_radix(raw, 16).unwrap().try_into().unwrap();
            let amount = Decimal::from_i128_with_scale(units as i128, 18);
            let mut event = deposit(hash, TOKEN.parse().unwrap(), amount, "w4");
            event.asset.ticker = "STAY".into();
            event.source = format!("Transfer в блоке {block}");
            let ticket = credit_watch::register(
                &core,
                deal,
                &VenueId("kucoin".into()),
                &event.venue,
                &event.asset,
                amount,
            );
            ticket.set_account(event.account.clone());
            ticket.set_chain(before());
            credit_event::on_change(&core, event).await;
            let hit = ticket.chain_hit();
            assert_eq!(hit.is_some(), enabled, "{deal}");
            if let Some(hit) = &hit {
                assert_eq!(hit.tx_hash.as_deref(), Some(hash));
                assert_eq!(hit.delta, amount);
            }
            println!(
                "STAY_REPLAY {deal} chain_credit={enabled} credited_before_sender_status={}",
                hit.is_some()
            );
        }
    }
}
