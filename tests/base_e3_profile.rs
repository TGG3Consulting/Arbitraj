//! Поиск LB до получения списка индексатора на записанных рынках Base и BSC.
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes};
use arb::{
    settings::scope::Scope,
    venues::bsc_dex::{
        ctx::Place,
        pools::{abi_lb::ILBFactory, factory_calls, families},
    },
};
use flate2::read::GzDecoder;
use serde_json::{Value, json};

#[path = "common/first_run_profiles.rs"]
mod first_run_profiles;
mod native_common;

fn archive(bytes: &[u8]) -> Value {
    serde_json::from_reader(GzDecoder::new(bytes)).unwrap()
}

#[tokio::test]
async fn recorded_lb_factories_keep_their_discovery_and_implementation_requirements() {
    let e3 = archive(include_bytes!(
        "data/first_run_dex_2026-11-26/e3_base_wallets.json.gz"
    ));
    let sector = archive(include_bytes!(
        "data/first_run_dex_2026-11-26/sectorone-22-base.json.gz"
    ));
    let sector = &sector["directory"];
    let bsc: Value =
        serde_json::from_str(include_str!("data/lb_v21_2026-11-15/probe.json")).unwrap();
    let bsc_id = bsc["geckoterminal"]["dex"].as_str().unwrap();

    for (id, source, x, y) in [
        (
            "e3-base",
            &e3,
            &e3["markets"][0]["x"],
            &e3["markets"][0]["y"],
        ),
        (
            "sectorone-base",
            sector,
            &sector["markets"][0]["tokens"][0],
            &sector["markets"][0]["tokens"][1],
        ),
        (
            bsc_id,
            &bsc,
            &bsc["pools"][0]["tokenX"],
            &bsc["pools"][0]["tokenY"],
        ),
    ] {
        let mut ctx = native_common::ctx(&format!("lb-discovery-{id}")).await;
        let settings = if id == bsc_id {
            json!({"dex.kind": "lb_v21", "dex.factory": source["factory"]})
        } else {
            ctx.place = Place {
                id: "base_dex",
                all: &["base_dex"],
            };
            first_run_profiles::named(8453, id).unwrap()["settings"].clone()
        };
        let scope = Scope::Dex(id.to_owned());
        for (key, value) in settings.as_object().unwrap() {
            ctx.settings
                .set(key, scope.clone(), Some(value.clone()), "test")
                .await
                .unwrap();
        }
        ctx.settings
            .set("dex.pool_enabled", scope, Some(json!(true)), "test")
            .await
            .unwrap();
        let family = families::classify(&ctx, id).unwrap();
        let required = families::required(&ctx, family);
        assert_eq!(
            required.contains(&"dex.proxy_runtime_hashes"),
            id == "e3-base"
        );
        assert_eq!(
            required.contains(&"dex.factory_runtime_hash"),
            id == "e3-base"
        );
        assert_eq!(required.contains(&"dex.lb_implementations"), id != bsc_id);

        let token = |v: &Value| v.as_str().unwrap().parse::<Address>().unwrap();
        let (x, y) = (token(x), token(y));
        let plan = factory_calls::plan(&ctx, &[x], &[y], &Default::default()).unwrap();
        let calls: Vec<_> = plan
            .asks
            .into_iter()
            .zip(plan.calls)
            .filter(|(ask, _)| ask.family.id == id)
            .map(|(_, call)| call)
            .collect();
        if id == bsc_id {
            assert!(calls.is_empty());
            assert!(
                plan.notes
                    .iter()
                    .any(|note| note.starts_with(id) && note.contains("индексатора"))
            );
        } else {
            let calldata = ILBFactory::getAllLBPairsCall {
                tokenX: x,
                tokenY: y,
            }
            .abi_encode();
            assert_eq!(
                calls,
                vec![(token(&source["factory"]), Bytes::from(calldata))]
            );
        }
    }
}
