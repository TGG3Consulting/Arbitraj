#[tokio::test]
async fn recorded_treble_profiles_require_their_own_router_roles() {
    use crate::venues::bsc_dex::{
        hashflow::recovery_tests::{directory, parts},
        pools::{base_plugin_v1, families, per_block},
    };
    use flate2::read::GzDecoder;
    use serde_json::Value;

    let dir = directory("treble-profile-roles", 8453);
    let p = parts(&dir, 8453, alloy_primitives::Address::ZERO).await;
    let profiles: Vec<Value> =
        serde_json::from_str(include_str!("../../../../config/first_run_dex.json")).unwrap();
    for (archive, spot) in [
        (
            include_bytes!(
                "../../../../tests/data/first_run_dex_2026-11-26/base_algebra_treble-spot.json.gz"
            )
            .as_slice(),
            true,
        ),
        (
            include_bytes!(
                "../../../../tests/data/first_run_dex_2026-11-26/base_algebra_treble-v4.json.gz"
            )
            .as_slice(),
            false,
        ),
    ] {
        let data: Value = serde_json::from_reader(GzDecoder::new(archive)).unwrap();
        let id = data["id"].as_str().unwrap();
        let profile = &profiles
            .iter()
            .find(|r| r["id"] == id && r["chain_id"] == data["chain_id"])
            .unwrap()["settings"];
        let family = families::make(id, profile["dex.kind"].as_str().unwrap()).unwrap();
        assert_eq!(family.variant.treble_spot(), spot);
        assert_eq!(family.variant.exact_router(), spot);
        assert_eq!(family.variant.algebra_router(), spot);
        assert_eq!(
            families::required(&p.ctx, family).contains(&"dex.algebra_pool_deployer"),
            spot
        );
        if spot {
            assert_eq!(
                profile["dex.algebra_pool_deployer"],
                data["roles"]["deployer"]["address"]
            );
        }
        assert!(per_block::needs(family));
        assert_eq!(
            u64::from(base_plugin_v1::flags(family.variant)),
            data["global_state"][3].as_u64().unwrap()
        );
    }
}
