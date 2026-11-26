//! Четыре независимо зарегистрированных DEX: все 16 положений выключателей,
//! удержание только сети открытой сделки и её резерва на общем счёте биржи.
//! GitHub Actions, обычное ядро с временной SQLite, без сетевых запросов.
mod t489_common;
use arb::core::{
    chain, chain_play, deal::Step, feed_hold, ledger, registry_sync, types::ChainId, usdt_hold,
};
use arb::gateway::contract::VenueKind;
use arb::settings::scope::Scope;
use rust_decimal::Decimal;
use serde_json::json;
use t489_common::{deals, set, start, v};

#[tokio::test]
async fn four_networks_keep_only_the_active_deal_and_its_shared_reserve() {
    let core = start("net-489-four-settlement", true, false).await;
    let chains = [
        ("bsc_dex", "bsc"),
        ("arb_dex", "arbitrum"),
        ("base_dex", "base"),
        ("eth_dex", "ethereum"),
    ];
    for (id, tag) in &chains[2..] {
        core.inner
            .kinds
            .write()
            .unwrap()
            .push((v(id), VenueKind::Dex));
        set(&core, "dex.chain", Scope::Venue(id.to_string()), json!(tag)).await;
    }
    for mask in 0..16u8 {
        for (i, (id, _)) in chains.iter().enumerate() {
            set(
                &core,
                "venue.enabled",
                Scope::Venue(id.to_string()),
                json!(mask & (1 << i) != 0),
            )
            .await;
        }
        let active = chain_play::active_dexes(&core);
        for (i, (id, tag)) in chains.iter().enumerate() {
            let enabled = mask & (1 << i) != 0;
            assert_eq!(active.contains(&v(id)), enabled, "{mask} {id}");
            let canonical = (i != 0).then(|| ChainId(tag.to_string()));
            assert_eq!(
                chain::dex_of(
                    &core.inner.deps.settings,
                    &chain::dexes(&core),
                    canonical.as_ref()
                )
                .unwrap(),
                v(id)
            );
        }
    }
    for (id, _) in chains {
        set(
            &core,
            "venue.enabled",
            Scope::Venue(id.into()),
            json!(false),
        )
        .await;
    }
    for (i, (id, tag)) in chains.iter().enumerate() {
        let mut deal = deals::new_deal(id, id, (i != 0).then(|| ChainId(tag.to_string())));
        deal.plan = json!({"settlement_cycle":{"market":{"cex":"bingx"},"quote_cap":"100"}});
        deal.invested = Decimal::from(20);
        ledger::save(&core, &deal).await;
        assert_eq!(chain_play::active_dexes(&core), vec![v(id)]);
        assert!(!registry_sync::enabled(&core, &v(id)));
        let (assets, held) = feed_hold::want_disabled(&core, &v(id));
        assert!(held && assets.contains(&deal.direction.asset));
        assert_eq!(
            usdt_hold::held(&core, &v("bingx"), None).amount,
            Decimal::from(80)
        );
        for (other, _) in chains.iter().filter(|(other, _)| other != id) {
            assert!(feed_hold::want_disabled(&core, &v(other)).0.is_empty());
            assert!(usdt_hold::held(&core, &v(other), None).amount.is_zero());
        }
        deal.step = Step::Closed;
        ledger::save(&core, &deal).await;
        assert!(chain_play::active_dexes(&core).is_empty());
        assert!(usdt_hold::held(&core, &v("bingx"), None).amount.is_zero());
    }
}
