//! Ручной выбор рынка по конфигурации и опубликованным кадрам Ethereum. Все запросы заканчиваются до RPC.
use arb::{
    gateway::{contract::Venue, source_quote::Request},
    settings::scope::Scope,
    venues::bsc_dex::{
        BscDex,
        ctx::{Ctx, Place},
    },
};
use serde_json::{Value, json};

#[path = "common/first_run_profiles.rs"]
mod first_run_profiles;
mod native_common;

async fn set(ctx: &Ctx, name: &str, scope: &Scope, value: Value) {
    ctx.settings
        .set(name, scope.clone(), Some(value), "test")
        .await
        .unwrap();
}

async fn install(ctx: &Ctx, scope: &Scope, first_run_id: &str, builders: Value) {
    let settings = &first_run_profiles::named(1, first_run_id).unwrap()["settings"];
    for (key, value) in settings.as_object().unwrap() {
        set(ctx, key, scope, value.clone()).await;
    }
    set(ctx, "dex.pool_enabled", scope, json!(true)).await;
    set(ctx, "dex.propamm_slot_builders", scope, builders).await;
}

async fn refusal(dex: &BscDex, name: &str, frame: &Value, builder: &str) -> String {
    let order: Request = serde_json::from_value(json!({
        "input": frame["pairs"][0][0], "output": frame["pairs"][0][1],
        "market": frame["market"], "amount_raw": "1", "builder": builder
    }))
    .unwrap();
    match dex.source_quote(name, order).await {
        Err(error) => error.message,
        Ok(_) => panic!("Без плательщика котировка недопустима"),
    }
}

#[tokio::test]
async fn renamed_markets_share_the_wallet_and_request_profile_only_on_their_network() {
    let frames: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/prop_ethereum_builder_frames.json"
    ))
    .unwrap();
    let frame = |name| {
        frames["frames"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["name"] == name)
            .unwrap()
    };
    let (kaliber, taurus) = (frame("kaliber"), frame("taurus"));
    let mut ctx = native_common::ctx("renamed-lambda-slots").await;
    ctx.place = Place {
        id: "private_l1",
        all: &["bsc_dex", "private_l1"],
    };
    let bsc = ctx.for_place("bsc_dex");
    let venue = Scope::Venue(ctx.place.id.into());
    let bsc_venue = Scope::Venue(bsc.place.id.into());
    set(&ctx, "dex.default_chain", &Scope::Global, json!("bsc")).await;
    set(&ctx, "dex.chain", &bsc_venue, json!("bsc")).await;
    set(&ctx, "dex.evm_chain_id", &bsc_venue, json!(56)).await;
    set(&ctx, "dex.chain", &venue, json!("ethereum")).await;
    set(&ctx, "dex.evm_chain_id", &venue, frames["chain"].clone()).await;
    let named = Scope::Dex("first@ethereum".into());
    install(&ctx, &named, "kaliber-lambda-ethereum", json!(["titan"])).await;
    install(
        &ctx,
        &Scope::Dex("dexalot@ethereum".into()),
        "taurus-lambda-ethereum",
        json!(["buildernet", "titan"]),
    )
    .await;
    install(
        &ctx,
        &Scope::Dex("first".into()),
        "kaliber-lambda-ethereum",
        json!(["titan"]),
    )
    .await;

    let dex = BscDex::new(ctx.clone());
    let bsc_dex = BscDex::new(bsc);
    assert!(bsc_dex.slot_quote_sources().is_empty());
    assert!(
        refusal(&bsc_dex, "lambda:first", kaliber, "titan")
            .await
            .contains("в этой сети")
    );
    assert_eq!(
        serde_json::to_value(dex.slot_quote_sources()).unwrap(),
        json!({"first":["titan"],"dexalot":["buildernet","titan"]})
    );
    for (id, data, builder) in [
        ("first", kaliber, "titan"),
        ("lambda:first", kaliber, "titan"),
        ("lambda:dexalot", taurus, "buildernet"),
    ] {
        assert!(
            refusal(&dex, id, data, builder)
                .await
                .contains("плательщика")
        );
    }
    assert!(
        refusal(&dex, "first", taurus, "titan")
            .await
            .contains("другой адрес")
    );
    assert!(
        refusal(&dex, "dexalot", taurus, "titan")
            .await
            .contains("Dexalot:")
    );
    assert!(
        refusal(&dex, "lambda:first", kaliber, "buildernet")
            .await
            .contains("строителя")
    );
    assert!(
        refusal(&dex, "lambda:first@ethereum", kaliber, "titan")
            .await
            .contains("профиль PropAMM")
    );

    for (key, value) in [
        ("dex.propamm_price_slots", json!([])),
        ("dex.propamm_price_slots", json!(["bad-slot"])),
        ("dex.propamm_slot_builders", json!([])),
        ("dex.propamm_slot_builders", json!(["titan", " "])),
        ("dex.propamm_slot_builders", json!(["unknown"])),
        ("dex.kind", json!("lb_v21")),
    ] {
        let old = ctx.settings.get_exact(key, &named).unwrap();
        set(&ctx, key, &named, value).await;
        assert!(!dex.slot_quote_sources().contains_key("first"), "{key}");
        assert!(
            refusal(&dex, "lambda:first", kaliber, "titan")
                .await
                .contains("профиль PropAMM"),
            "{key}"
        );
        set(&ctx, key, &named, old).await;
    }
    set(&ctx, "dex.chain", &venue, json!("bsc")).await;
    assert!(dex.slot_quote_sources().is_empty());
    assert!(
        refusal(&dex, "lambda:first", kaliber, "titan")
            .await
            .contains("в этой сети")
    );
}
