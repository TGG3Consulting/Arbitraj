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
    assert!(Origin::parse(6, "rpc", "36667253").is_none());
    let (cursor, seed) = history_orders::seed(p, from).unwrap();
    assert_eq!(cursor.0, 47_495_779);
    assert_eq!(seed.len(), 363);
    assert!(!seed.contains_key(&removed));
    assert!(seed.contains_key(&active));
    assert!(history_orders::seed(Profile { chain: 1, ..p }, 1).is_none());
    logs.reverse();
    let delta = history_orders::changes(p.book, 0, cursor.0, &mut logs).unwrap();
    assert_eq!(delta[&active].as_ref(), seed.get(&active));
    assert_eq!(delta[&removed], None);
    logs[0].removed = true;
    assert!(history_orders::changes(p.book, 0, cursor.0, &mut logs).is_none());
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
        let delta = history_orders::changes(p.book, n, n, &mut [l]).unwrap();
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
    let saved = history_store::load(&reopened.ctx, ns).await.unwrap();
    let (cursor, orders) = saved.valid.unwrap();
    assert_eq!(cursor, *cursors.last().unwrap());
    assert!(orders.contains_key(&active));
    assert!(!orders.contains_key(&removed));
    // Контролируемый откат к записанному первому событию; принятый новый курсор нужен и здесь.
    let (_, _, _, mut original) = records();
    let first = cursors[0];
    let initial = history_orders::changes(p.book, first.0, first.0, &mut original[..1]).unwrap();
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
    let (reset, orders) = history_store::load(&reopened.ctx, ns)
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
