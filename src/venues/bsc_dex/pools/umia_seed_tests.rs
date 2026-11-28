//! Начальный проход списка Umia на реальном профиле Base, без индексатора и ранее увиденных Initialize.
use super::*;
use crate::{settings::scope::Scope, venues::bsc_dex::hashflow::recovery_tests};
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn archive(bytes: &[u8]) -> Value {
    serde_json::from_reader(flate2::read::GzDecoder::new(bytes)).unwrap()
}

#[tokio::test]
async fn initial_candidates_include_both_funded_vaults_of_the_migrated_venture() {
    let catalog = archive(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/umia_base_directory.json.gz"
    ));
    let evidence = archive(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/umia_base_wallets.json.gz"
    ));
    let history = archive(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/umia_base_history.json.gz"
    ));
    let profiles: Vec<Value> = serde_json::from_str(include_str!(
        "../../../../config/first_run_dex.json"
    )).unwrap();
    let profile = profiles.iter().find(|p| p["id"] == "umia-base").unwrap();
    let chain = profile["chain_id"].as_u64().unwrap();
    let dir = recovery_tests::directory("umia-initial-markets", chain);
    let p = recovery_tests::parts(&dir, chain, Address::ZERO).await;
    let ctx = &p.ctx;
    let store = &ctx.settings;
    store.set("dex.default_chain", Scope::Global, Some(json!("bsc")), "test").await.unwrap();
    store.set("dex.chain", Scope::Venue(ctx.place.id.into()), Some(json!("base")), "test").await.unwrap();
    store.set("dex.evm_chain_id", Scope::Venue(ctx.place.id.into()), Some(json!(chain)), "test").await.unwrap();
    let scope = Scope::Dex("umia-base@base".into());
    for (name, setting) in profile["settings"].as_object().unwrap() {
        store.set(name, scope.clone(), Some(setting.clone()), "test").await.unwrap();
    }

    let ventures = catalog["rows"].as_array().unwrap();
    let mut coins = HashMap::new();
    for venture in ventures {
        let token = venture["token"].as_str().unwrap().parse().unwrap();
        coins.insert(token, venture["name"].as_str().unwrap().to_owned());
    }
    let migrated = ventures.iter().find(|v| v["terms"]["migrator"].is_string()).unwrap();
    let money = migrated["money"].as_str().unwrap().parse().unwrap();
    let current = migrated["id"].as_str().unwrap();
    let retired = history["retired"][0]["id"].as_str().unwrap();
    let mut out = Extra::default();
    extend(ctx, &coins, &[money], &mut out);
    let found: BTreeSet<_> = out.found.values().flatten().map(|row| {
        assert_eq!(row.quote, money);
        assert_eq!(row.dex_id, "umia-base@base");
        assert!(row.reserve_usd.is_none());
        row.pool.as_str()
    }).collect();
    let paid: BTreeSet<_> = evidence["markets"].as_array().unwrap().iter()
        .filter(|m| m["cases"].as_array().unwrap().iter().any(|c|
            c["buy"]["stage"] == "accepted" && c["sell"]["stage"] == "accepted"))
        .map(|m| m["pool"].as_str().unwrap()).collect();
    assert_eq!(found, paid);
    assert_eq!(found.len(), 3);
    let asset: Address = migrated["token"].as_str().unwrap().parse().unwrap();
    let same_pair: BTreeSet<_> = out.found[&asset].iter().map(|row| row.pool.as_str()).collect();
    assert_eq!(same_pair, BTreeSet::from([current, retired]));
    assert_eq!(out.calls, 0);
    let mut no_cash_pair = Extra::default();
    extend(ctx, &coins, &[Address::ZERO], &mut no_cash_pair);
    assert!(no_cash_pair.found.is_empty());

    store.set("dex.pool_enabled", scope, Some(json!(false)), "test").await.unwrap();
    let mut disabled = Extra::default();
    extend(ctx, &coins, &[money], &mut disabled);
    assert!(disabled.found.is_empty());
}
