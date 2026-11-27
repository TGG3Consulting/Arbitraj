//! Первичные события Initialize двух размещений Base, записанные хеши ролей и контроль семейства BSC.
use super::*;

pub(super) fn check_roles(roles: &[Role], d: &Value) {
    let archived: Value = serde_json::from_str(include_str!(
        "../../../../tests/data/first_run_dex_2026-11-26/basestonk_base_roles.json"
    )).unwrap();
    assert_eq!(archived["tag"], d["tag"]);
    for row in archived["hook_roles"].as_array().unwrap() {
        for (field, hash, kind) in [("hook", "hook_runtime", "hook"), ("launcher", "launcher_runtime", "factory")] {
            let address = addr(&row[field]);
            let recorded: B256 = row[hash].as_str().unwrap().parse().unwrap();
            assert!(roles.iter().any(|r| r.kind == kind && r.address == address && r.hash == recorded));
        }
    }
}

#[test]
fn verified_launch_keys_and_bsc_stableswap_have_distinct_admission() {
    assert!(!family_variant::of("launch_umia").basestonk());
    let bsc: Value = serde_json::from_str(include_str!(
        "../../../../tests/data/bsc_pancake_stable_receipt_2026-11-27.json"
    )).unwrap();
    assert_eq!(bsc["chainId"], 56);
    let event = bsc["logs"].as_array().unwrap().iter()
        .find(|log| log["logIndex"] == bsc["sourceLogIndex"]).unwrap();
    assert_eq!(event["address"], bsc["pool"]);
    assert_eq!(event["topics"][0], format!("{:#x}",
        keccak256("TokenExchange(address,uint256,uint256,uint256,uint256)")));
    assert!(!family_variant::of("pancake_stableswap").basestonk());

    let directory = snapshot(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/basestonk_base_directory.json.gz"
    ));
    // PoolManager Base, Initialize: блок 50086809, logIndex 3276; блок 53514556, logIndex 245.
    // https://basescan.org/tx/0xee13974cc1345d094589678d65b75961ad8098e8cdedf2774ff7400605655b49#eventlog
    // https://basescan.org/tx/0x0f7bf2c07cfd1351625299b0db55b0c498e8b84bd09f04d1f953b2801aee402e#eventlog
    for (id, fee, hook) in [
        ("0x383c16b06171eb5c08bd8440cc5a4363be99967ab1a82147286c130efec212ce", 3000u32, "0x03d2434d5a9ab7fb46bd3c7956a7c62e0cd46044"),
        ("0x0278fbf73060b4b6544a3eef8f3fe3c16e334d1d03fcc06736b802e254789cf6", 0u32, "0x5c999109609f9f857f4d58240ccb12bf219ee044"),
    ] {
        let row = directory["markets"].as_array().unwrap().iter().find(|row| row["id"] == id).unwrap();
        assert_eq!(row["key"][2], fee);
        assert_eq!(row["key"][3], 60);
        assert_eq!(row["key"][4], hook);
        let key = basestonk::key((addr(&row["token"]), addr(&row["pair"])), addr(&row["hook"]), fee);
        assert_eq!(key.id(), id.parse::<B256>().unwrap());
        let PoolKey::Uniswap(k) = key else { panic!("ожидался ключ V4") };
        assert!(basestonk::matches_key(&k));
        let mut altered = k.clone();
        altered.fee = 500;
        assert!(!basestonk::matches_key(&altered));
        altered = k.clone();
        altered.tick_spacing = alloy_primitives::aliases::I24::try_from(1).unwrap();
        assert!(!basestonk::matches_key(&altered));
        altered = k.clone();
        altered.hooks = Address::with_last_byte(0x44);
        assert!(!basestonk::matches_key(&altered));
    }
}
