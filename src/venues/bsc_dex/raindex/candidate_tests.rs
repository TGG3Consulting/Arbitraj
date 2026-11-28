//! Первичная выборка даёт только программы. Текущий live и денежная проверка принадлежат книге и кошельку.
use super::*;
use crate::venues::bsc_dex::raindex::{history_candidates::{Indexed, View}, source::Raw};
use std::collections::HashSet;

#[test]
fn initial_candidates_are_scoped_deduplicated_and_never_advance_the_cursor() {
    let archive: Value = serde_json::from_reader(GzDecoder::new(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/raindex_base_deployment_balances.json.gz"
    ).as_slice())).unwrap();
    let height = archive["height"].as_u64().unwrap();
    let anchor = from_value(archive["selector"]["blockHash"].clone()).unwrap();
    let mut total = 0;
    for g in archive["order_samples"].as_array().unwrap() {
        let book = archive["books"].as_array().unwrap().iter().find(|v| v["book"]==g["book"]).unwrap();
        let p = Profile {
            chain: 8453, version: book["version"].as_u64().unwrap() as u8,
            book: from_value(book["book"].clone()).unwrap(),
            book_hash: from_value(book["code_hash"].clone()).unwrap(),
            module: Address::ZERO, module_hash: B256::ZERO,
        };
        let from = book["start_block"].as_u64().unwrap();
        let open: HashSet<B256> = g["orders"].as_array().unwrap().iter()
            .filter(|v| v["exists"]==true).map(|v| from_value(v["hash"].clone()).unwrap()).collect();
        let empty = Indexed::default();
        let view = View::for_book(p, from, height, &empty);
        assert_eq!(view.iter().map(|(h,_)| *h).collect::<HashSet<_>>(), open);
        total += open.len();
        assert!(empty.cursor.is_none() && empty.rows.is_empty());
        assert_eq!(View::for_book(p, from, height-1, &empty).iter().count(), 0);
        assert_eq!(View::for_book(p, from+1, height, &empty).iter().count(), 0);
        assert_eq!(View::for_book(Profile { chain: 1, ..p }, from, height, &empty).iter().count(),0);
        assert_eq!(View::for_book(Profile {book_hash:B256::ZERO, ..p}, from, height, &empty).iter().count(),0);
        if let Some((hash, bytes)) = view.iter().next() {
            let rows: Raw = [(*hash,bytes.clone())].into_iter().collect();
            let partial_scan = Indexed {cursor:Some((height-1, B256::ZERO)), rows:Arc::new(rows)};
            assert_eq!(View::for_book(p, from, height, &partial_scan).iter().count(),open.len());
        }
        // Вся история дошла до контрольного блока, оператором учтены и снятия.
        // В журнале может остаться пустой набор. Старая подсказка не возвращает удалённый ордер.
        let scanned = Indexed {cursor:Some((height,anchor)), rows:Arc::new(Raw::new())};
        assert_eq!(View::for_book(p, from, height, &scanned).iter().count(),0);
    }
    assert_eq!(total,16);
}

#[test]
fn archived_removed_programs_fail_current_registration_before_a_ticket_exists() {
    let removed: Vec<Value> = serde_json::from_reader(GzDecoder::new(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/raindex_removed_liveness.json.gz"
    ).as_slice())).unwrap();
    for row in removed {
        let v = row["version"].as_u64().unwrap() as u8;
        let hash = from_value(row["hash"].clone()).unwrap();
        let bytes: Bytes = from_value(row["order"].clone()).unwrap();
        let reply: Bytes = from_value(row["reply"].clone()).unwrap();
        let order = wire::Order::decode(v,&bytes,hash).unwrap();
        if v == 3 {
            assert!(!legacy::registered(&reply));
        } else {
            assert!(order.price(&reply).is_none());
        }
    }
}
