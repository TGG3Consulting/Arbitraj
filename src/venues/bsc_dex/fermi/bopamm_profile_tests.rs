//! Допуск кошелька в собственной сети по ролям из канонического каталога bopAMM.
use super::*;
use crate::venues::bsc_dex::hashflow::recovery_tests::{directory, parts};
use flate2::read::GzDecoder;
use serde_json::json;

#[tokio::test]
async fn archived_bopamm_deployment_requires_local_network_and_roles() {
    let bytes =
        include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/bopamm_ethereum.json.gz");
    let archive: Value = serde_json::from_reader(GzDecoder::new(bytes.as_slice())).unwrap();
    let catalog = &archive["directory"];
    assert_eq!(catalog["height"], 26_494_988);
    let roles = recommendations::venue_value("bopamm.roles", "eth_dex").unwrap();
    let rows = json!(roles.split(',').collect::<Vec<_>>());
    let tmp = directory("bopamm-profile", evm_profiles::ETHEREUM_MAINNET);
    let p = parts(&tmp, evm_profiles::ETHEREUM_MAINNET, Address::ZERO).await;
    let settings = &p.ctx.settings;
    for (venue, id, chain, expected) in [
        ("eth_dex", 1, "ethereum", true),
        ("renamed_dex", 1, "ethereum", true),
        ("base_dex", 8453, "base", false),
        ("bsc_dex", 56, "bsc", false),
    ] {
        let scope = Scope::Venue(venue.to_owned());
        // У прежнего кошелька BSC без идентификаторов и у площадки без своих ролей формы нет.
        assert!(!configured(settings, venue));
        settings
            .set("dex.evm_chain_id", scope.clone(), Some(json!(id)), "test")
            .await
            .unwrap();
        settings
            .set("dex.chain", scope.clone(), Some(json!(chain)), "test")
            .await
            .unwrap();
        assert!(!configured(settings, venue));
        settings
            .set("bopamm.roles", scope.clone(), Some(rows.clone()), "test")
            .await
            .unwrap();
        assert_eq!(configured(settings, venue), expected);
        if expected {
            let (network, profile) = Profile::read(settings, venue).unwrap();
            assert_eq!(network, evm_profiles::ETHEREUM_MAINNET);
            assert!(profile.module.is_none());
            let market = &catalog["roles"][format!("{:#x}", profile.market.address)];
            assert_eq!(market["runtime"], json!(profile.market.hash));
            assert_eq!(market["pricing"], json!(profile.pricing.address));
            for r in [profile.pricing, profile.registry] {
                assert_eq!(
                    catalog["pricing_roles"][format!("{:#x}", r.address)],
                    json!(r.hash)
                );
            }
            // Старая проверка «непустой список» принимала и произвольный текст.
            for invalid in [json!(["garbage"]), json!([rows[0], rows[1], rows[1]])] {
                settings
                    .set("bopamm.roles", scope.clone(), Some(invalid), "test")
                    .await
                    .unwrap();
                assert!(!configured(settings, venue));
            }
        }
    }
    drop(p);
    let _ = std::fs::remove_dir_all(tmp);
}

#[tokio::test]
async fn paid_wallet_requires_its_own_builder_permission_and_physical_usdt() {
    use super::super::{Builder, bop_cash, bop_versions};
    let data = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/bopamm_ethereum_cash_wallet.json.gz"
    );
    let archive: Value = serde_json::from_reader(GzDecoder::new(data.as_slice())).unwrap();
    let case = &archive["frames"][0]["cases"][0]["compound"];
    let executor: Address = case["executor"].as_str().unwrap().parse().unwrap();
    let roles = recommendations::venue_value("bopamm.roles", "eth_dex").unwrap();
    let mut rows: Vec<_> = roles.split(',').map(str::to_owned).collect();
    rows.push(format!(
        "cash_module {executor:#x} {:#x}",
        bop_versions::CASH
    ));
    for (id, chain) in [(1, "ethereum"), (56, "bsc")] {
        let tmp = directory("bop-cash-opt-in", id);
        let p = parts(&tmp, id, Address::ZERO).await;
        let ctx = &p.ctx;
        let scope = Scope::Venue(ctx.place.id.to_owned());
        for (key, value) in [
            ("dex.evm_chain_id", json!(id)),
            ("dex.chain", json!(chain)),
            ("bopamm.roles", json!(rows)),
        ] {
            ctx.settings
                .set(key, scope.clone(), Some(value), "test")
                .await
                .unwrap();
        }
        assert!(bop_cash::configured(ctx, bop_cash::USDT).is_empty());
        ctx.settings
            .set(
                "bopamm.slot_builders",
                scope,
                Some(json!(["titan"])),
                "test",
            )
            .await
            .unwrap();
        assert!(bop_cash::configured(ctx, bop_cash::USDC).is_empty());
        let paths = bop_cash::configured(ctx, bop_cash::USDT);
        if id == 1 {
            assert_eq!(paths.len(), 2);
            assert!(
                paths
                    .iter()
                    .all(|(b, r)| *b == Builder::Titan && r.module.address == executor)
            );
            assert_eq!(paths.iter().filter(|(_, r)| r.bridge).count(), 1);
            let previous = paths[0].1;
            assert!(previous.permitted(ctx, Builder::Titan));
            let mut changed = rows.clone();
            changed.pop();
            changed.push(format!(
                "cash_module {:#x} {:#x}",
                Address::repeat_byte(53),
                bop_versions::CASH
            ));
            ctx.settings
                .set(
                    "bopamm.roles",
                    Scope::Venue(ctx.place.id.into()),
                    Some(json!(changed)),
                    "test",
                )
                .await
                .unwrap();
            assert!(!previous.permitted(ctx, Builder::Titan));
            let next = bop_cash::configured(ctx, bop_cash::USDT);
            assert!(
                next.iter()
                    .all(|(b, r)| r.module.address != executor && r.permitted(ctx, *b))
            );
        } else {
            assert!(paths.is_empty());
        }
        drop(p);
        let _ = std::fs::remove_dir_all(tmp);
    }
}
