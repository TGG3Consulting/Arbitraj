//! Первичные Add/Remove издателя, перенос от снимка и гонка записи старого курсора.
use super::*;
use crate::storage::{db::Db, venue_state};
use crate::venues::bsc_dex::raindex::{history_orders, history_store};

fn records() -> (Profile, B256, B256, Vec<Log>) {
    let source: Value = serde_json::from_reader(GzDecoder::new(
        include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/raindex_base_v5_seed.json.gz"
        )
        .as_slice(),
    ))
    .unwrap();
    let sample: Value = serde_json::from_reader(GzDecoder::new(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/raindex_base_v5_history_control.json.gz"
    ).as_slice())).unwrap();
    let profile = Profile {
        chain: source["chain_id"].as_u64().unwrap(),
        version: source["version"].as_u64().unwrap() as u8,
        book: from_value(source["book"].clone()).unwrap(),
        book_hash: from_value(source["book_hash"].clone()).unwrap(),
        module: Address::ZERO,
        module_hash: B256::ZERO,
    };
    (
        profile,
        from_value(sample["removed"].clone()).unwrap(),
        from_value(sample["active"].clone()).unwrap(),
        from_value(sample["logs"].clone()).unwrap(),
    )
}

#[test]
fn historical_book_recovers_open_orders_and_ignores_a_removed_order() {
    let (p, removed, active, mut logs) = records();
    use crate::venues::bsc_dex::raindex::profile::Origin;
    let Some(Origin::Rpc(from)) = Origin::parse(5, "rpc", "36667253") else {
        panic!("исходный блок V5");
    };
    assert!(Origin::parse(5, "rpc", "https://example.org").is_none());
    assert!(Origin::parse(5, "rpc", "0").is_none());
    assert!(Origin::parse(8, "rpc", "36667253").is_none());
    let (cursor, seed) = history_orders::seed(p, from).unwrap();
    assert_eq!(cursor.0, 47_495_779);
    assert_eq!(seed.len(), 363);
    assert!(!seed.contains_key(&removed));
    assert!(seed.contains_key(&active));
    assert!(history_orders::seed(Profile { chain: 1, ..p }, 1).is_none());
    logs.reverse();
    let delta = history_orders::changes(p, 0, cursor.0, &mut logs).unwrap();
    assert_eq!(delta[&active].as_ref(), seed.get(&active));
    assert_eq!(delta[&removed], None);
    logs[0].removed = true;
    assert!(history_orders::changes(p, 0, cursor.0, &mut logs).is_none());
}

#[tokio::test]
async fn old_history_reset_cannot_replace_newer_confirmed_orders() {
    let (p, removed, active, logs) = records();
    let ns = "raindex-v5-regression";
    let dir = crate::venues::bsc_dex::hashflow::recovery_tests::directory("raindex-history", 8453);
    let db = Db::open(&dir.join("state.sqlite")).unwrap();
    let write = |prev, cur, reset, delta| {
        history_store::commit(&db, "base_dex", ns, prev, cur, reset, delta)
    };
    let mut prev = None;
    let mut cursors = Vec::new();
    for l in logs {
        let n = l.block_number.unwrap();
        let c = (n, l.block_hash.unwrap());
        let delta = history_orders::changes(p, n, n, &mut [l]).unwrap();
        assert!(write(prev, c, false, delta).await.unwrap());
        prev = Some(serde_json::json!(c).to_string());
        cursors.push(c);
    }
    let stale = Some(serde_json::json!(cursors[0]).to_string());
    assert!(
        !write(stale, cursors[0], true, history_orders::Delta::new())
            .await
            .unwrap()
    );
    let rows = venue_state::list(&db, "base_dex", ns, "").await.unwrap();
    assert!(rows.iter().any(|(k, _)| k == &format!("{active:#x}")));
    assert!(!rows.iter().any(|(k, _)| k == &format!("{removed:#x}")));
    assert_eq!(
        rows.iter().find(|(k, _)| k == "cursor").map(|(_, v)| v),
        prev.as_ref()
    );
    drop(db);
    let reopened =
        crate::venues::bsc_dex::hashflow::recovery_tests::parts(&dir, 8453, Address::ZERO).await;
    let saved = history_store::load(&reopened.ctx, ns, p.version).await.unwrap();
    let (cursor, orders) = saved.valid.unwrap();
    assert_eq!(cursor, *cursors.last().unwrap());
    assert!(orders.contains_key(&active));
    assert!(!orders.contains_key(&removed));
    // Контролируемый откат к записанному первому событию; принятый новый курсор нужен и здесь.
    let (_, _, _, mut original) = records();
    let first = cursors[0];
    let initial = history_orders::changes(p, first.0, first.0, &mut original[..1]).unwrap();
    assert!(
        history_store::commit(
            reopened.ctx.journal.db(),
            "base_dex",
            ns,
            saved.expected,
            first,
            true,
            initial
        )
        .await
        .unwrap()
    );
    let (reset, orders) = history_store::load(&reopened.ctx, ns, p.version)
        .await
        .unwrap()
        .valid
        .unwrap();
    assert_eq!(reset, first);
    assert!(orders.contains_key(&removed));
    assert!(!orders.contains_key(&active));
    drop(reopened);
    std::fs::remove_dir_all(dir).unwrap();
}

fn multiversion_samples() -> Vec<Value> {
    serde_json::from_reader(GzDecoder::new(
        include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/raindex_multiversion_history_events.json.gz"
        )
        .as_slice(),
    ))
    .unwrap()
}

fn sample_profile(row: &Value) -> Profile {
    Profile {
        chain: row["chain"].as_u64().unwrap(),
        version: row["version"].as_u64().unwrap() as u8,
        book: from_value(row["book"].clone()).unwrap(),
        book_hash: B256::ZERO,
        module: Address::ZERO,
        module_hash: B256::ZERO,
    }
}

#[test]
fn public_events_follow_each_book_abi_and_reject_a_forged_order_hash() {
    let mut covered = std::collections::HashSet::new();
    for row in multiversion_samples() {
        let p = sample_profile(&row);
        let added = row["added"].as_bool().unwrap();
        let hash: B256 = from_value(row["hash"].clone()).unwrap();
        let mut log: Log = from_value(row["log"].clone()).unwrap();
        let height = log.block_number.unwrap();
        let delta = history_orders::changes(p, height, height, std::slice::from_mut(&mut log))
            .unwrap();
        assert_eq!(delta.len(), 1);
        assert_eq!(delta[&hash].is_some(), added);
        if let Some(bytes) = &delta[&hash] {
            assert!(wire::Order::decode(p.version, bytes, hash).is_some());
        }
        // Порядок параметров в двух событиях V3 отличается от V4–V7.
        let index = match (p.version, added) {
            (3, true) => 3,
            (3, false) => 2,
            _ => 1,
        };
        let mut corrupt = log.inner.data.data.to_vec();
        corrupt[index * 32] ^= 1;
        log.inner.data.data = Bytes::from(corrupt);
        assert!(history_orders::changes(p, height, height, &mut [log]).is_none());
        covered.insert((p.chain, p.version, added));
    }
    for v in [3, 4, 6] {
        for add in [false, true] {
            assert!(covered.iter().any(|&(_, version, added)| (version, added) == (v, add)));
        }
    }
    assert!(covered.iter().any(|&(chain, _, _)| chain == 8453));
    assert!(covered.iter().any(|&(_, version, added)| version == 7 && added));
}

#[tokio::test]
async fn persisted_history_uses_the_book_order_encoding_after_reopen() {
    let dir = crate::venues::bsc_dex::hashflow::recovery_tests::directory("raindex-generations", 8453);
    let db = Db::open(&dir.join("state.sqlite")).unwrap();
    let mut saved = std::collections::HashMap::new();
    for row in multiversion_samples() {
        let p = sample_profile(&row);
        if !row["added"].as_bool().unwrap() {
            continue;
        }
        let std::collections::hash_map::Entry::Vacant(slot) = saved.entry(p.version) else {
            continue;
        };
        let mut log: Log = from_value(row["log"].clone()).unwrap();
        let at = log.block_number.unwrap();
        let cursor = (at, log.block_hash.unwrap());
        let delta = history_orders::changes(p, at, at, std::slice::from_mut(&mut log)).unwrap();
        let namespace = format!("history-sample-v{}", p.version);
        let hash: B256 = from_value(row["hash"].clone()).unwrap();
        assert!(history_store::commit(&db, "base_dex", &namespace, None, cursor, true, delta)
            .await.unwrap());
        slot.insert((cursor, hash, namespace));
    }
    drop(db);
    let reopened = crate::venues::bsc_dex::hashflow::recovery_tests::parts(&dir, 8453, Address::ZERO).await;
    for (version, (expected, hash, namespace)) in saved {
        let (cursor, rows) = history_store::load(&reopened.ctx, &namespace, version)
            .await.unwrap().valid.unwrap();
        assert_eq!(cursor, expected);
        assert!(rows.contains_key(&hash));
    }
    drop(reopened);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn fixed_output_samples_apply_only_to_the_confirmed_base_v4_binary() {
    let fixture: Value = serde_json::from_slice(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/raindex_base_v4_quote_interfaces.json"
    )).unwrap();
    let rows = multiversion_samples();
    let ceiling = U256::from(42);
    for book in fixture["books"].as_array().unwrap() {
        let legacy_abi = !book["external_quote"].as_bool().unwrap();
        let sample = rows.iter().find(|s| {
            s["chain"] == 8453 && s["book"] == book["book"] && (!legacy_abi || s["added"] == true)
        }).unwrap();
        let mut profile = sample_profile(sample);
        profile.book_hash = from_value(book["hash"].clone()).unwrap();
        assert_eq!(legacy::output_samples(profile), legacy_abi);
        assert_eq!(book["quote_push4"], !legacy_abi);
        assert_eq!(book["take"], "0x0997c4a0");
        assert_eq!(book["exists"], "0x2cb77e9f");
        if !legacy_abi {
            continue;
        }
        assert!(!legacy::output_samples(Profile { chain: 1, ..profile }));
        assert!(!legacy::output_samples(Profile { book_hash: B256::ZERO, ..profile }));
        assert!(legacy::budget_allowed(profile, Some(ceiling), ceiling));
        assert!(!legacy::budget_allowed(profile, Some(ceiling), ceiling + U256::ONE));
        assert!(!legacy::budget_allowed(profile, None, U256::ONE));
        let hash: B256 = from_value(sample["hash"].clone()).unwrap();
        let mut log: Log = from_value(sample["log"].clone()).unwrap();
        let at = log.block_number.unwrap();
        let delta = history_orders::changes(profile, at, at, std::slice::from_mut(&mut log)).unwrap();
        let order = wire::Order::decode(4, delta[&hash].as_ref().unwrap(), hash).unwrap();
        let action = order.take(0, 0, ceiling, 6, true, B256::repeat_byte(255)).unwrap();
        let call = wire::takeOrders2Call::abi_decode_validate(&action).unwrap();
        assert_eq!(call.config.minimumInput, ceiling);
        assert_eq!(call.config.maximumInput, ceiling);
        assert_eq!(call.config.maximumIORatio, U256::MAX);
        assert_eq!(call.config.orders.len(), 1);
        assert_eq!(call.config.orders[0].order.owner, order.owner());
        assert!(order.take(0, 0, ceiling, 6, false, B256::repeat_byte(255)).is_none());
    }
}
