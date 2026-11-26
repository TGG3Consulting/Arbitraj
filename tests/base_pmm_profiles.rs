//! Привязывает настройки портативных профилей PMM к байткоду и EIP-1967 на одном архивном блоке.
use serde_json::{Value, json};

fn snapshot(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap()
}
#[path = "common/first_run_profiles.rs"]
mod first_run_profiles;
fn profile(id: &str) -> &'static Value {
    first_run_profiles::named(8453, id).unwrap()
}

#[test]
fn runtime_roles_and_implementations_match_independent_archives() {
    let e = snapshot(include_str!(
        "data/first_run_dex_2026-11-26/elfomofi_base.json"
    ));
    let l = snapshot(include_str!("data/first_run_dex_2026-11-26/lunarbase.json"));
    let b = snapshot(include_str!(
        "data/first_run_dex_2026-11-26/baibai_base.json"
    ));
    for (id, archive) in [("elfomofi", &e), ("lunarbase", &l), ("baibai", &b)] {
        assert_eq!(archive["selector"]["requireCanonical"], true);
        assert!(!first_run_profiles::recommended(8453, id).is_empty());
        assert_eq!(
            archive["selector"]["blockHash"],
            profile(id)["verified_module"]["block_hash"]
        );
    }
    let es = &profile("elfomofi")["settings"];
    for (field, key) in [
        ("dex.pmm_pool", "router"),
        ("dex.pool_implementation", "pricing_implementation"),
        ("dex.oracle", "pricing"),
        ("dex.vault", "vault"),
    ] {
        assert_eq!(es[field], e[key]);
    }
    assert_eq!(
        es["dex.pmm_runtime_hashes"],
        json!([
            e["router_runtime"],
            e["pricing_code"],
            e["pricing_impl_code"]
        ])
    );
    for (key, role) in [
        ("dex.pmm_pricing_offsets", "pricing"),
        ("dex.pmm_vault_offsets", "vault"),
    ] {
        let offsets: Vec<u64> = es[key]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().parse().unwrap())
            .collect();
        assert_eq!(json!(offsets), e["immutable_layout"][role]);
    }
    let ls = &profile("lunarbase")["settings"];
    let pool = l["markets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["address"] == ls["dex.pmm_pool"])
        .unwrap();
    assert_eq!(ls["dex.pool_implementation"], pool["implementation"]);
    assert_eq!(
        ls["dex.pmm_runtime_hashes"],
        json!([
            pool["code"]["keccak256"],
            pool["implementation_code"]["keccak256"]
        ])
    );
    let bs = &profile("baibai")["settings"];
    let deps = &b["deployments"];
    let mut hashes = vec![deps["entry"]["code"]["keccak256"].clone()];
    for (role, field) in [
        ("entry", "dex.pmm_pool"),
        ("book", "dex.oracle"),
        ("custodian", "dex.vault"),
    ] {
        let d = &deps[role];
        assert_eq!(bs[field], d["address"]);
        assert_eq!(hashes[0], d["code"]["keccak256"]);
        hashes.push(d["implementation_code"]["keccak256"].clone());
    }
    assert_eq!(bs["dex.pmm_runtime_hashes"], json!(hashes));
    assert_eq!(
        bs["dex.pool_implementation"],
        deps["entry"]["implementation"]
    );
    assert_eq!(
        bs["dex.pmm_aux_implementations"],
        json!([
            deps["book"]["implementation"],
            deps["custodian"]["implementation"]
        ])
    );
}

mod native_common;

#[tokio::test]
async fn factory_scan_tracks_owner_changed_market_and_tokens() {
    use alloy_primitives::Address;
    use arb::settings::scope::Scope;
    use arb::venues::bsc_dex::pools::factory_calls;
    let ctx = native_common::ctx("reconfigured-elfomo-profile").await;
    let id = "custom-elfomo";
    let scope = Scope::Dex(id.into());
    let first = &profile("elfomofi")["settings"];
    for (key, value) in first.as_object().unwrap() {
        ctx.settings
            .set(key, scope.clone(), Some(value.clone()), "test")
            .await
            .unwrap();
    }
    ctx.settings
        .set("dex.pool_enabled", scope.clone(), Some(json!(true)), "test")
        .await
        .unwrap();
    let coin = Address::with_last_byte(21);
    let counter = Address::with_last_byte(22);
    for (key, value) in [
        ("dex.pmm_counter", json!(counter)),
        ("dex.pmm_base_tokens", json!([coin])),
    ] {
        ctx.settings
            .set(key, scope.clone(), Some(value), "test")
            .await
            .unwrap();
    }
    let scan = || {
        let plan = factory_calls::plan(&ctx, &[coin], &[counter], &Default::default()).unwrap();
        plan.asks
            .into_iter()
            .zip(plan.calls)
            .filter(|(ask, _)| ask.family.id == id)
            .map(|(_, call)| call.0)
            .collect::<Vec<_>>()
    };
    for pool in [Address::with_last_byte(23), Address::with_last_byte(24)] {
        ctx.settings
            .set("dex.pmm_pool", scope.clone(), Some(json!(pool)), "test")
            .await
            .unwrap();
        assert_eq!(scan(), vec![pool]);
    }
    ctx.settings
        .set("dex.pmm_pool", scope, Some(json!(coin)), "test")
        .await
        .unwrap();
    assert!(scan().is_empty());
}
