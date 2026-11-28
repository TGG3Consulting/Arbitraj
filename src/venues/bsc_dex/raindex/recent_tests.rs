//! Поток событий по явной RPC-книге не зависит от настроек истории пулов Uniswap.
use super::*;
use crate::settings::scope::Scope;
use crate::venues::bsc_dex::raindex::{
    history, history_candidates::{Indexed, View}, history_orders, history_store, source::Raw,
};

#[tokio::test]
async fn an_explicit_rpc_book_enables_only_its_chain_without_pool_scan_or_reporting() {
    let dir = crate::venues::bsc_dex::hashflow::recovery_tests::directory("raindex-selection", 8453);
    let parts = crate::venues::bsc_dex::hashflow::recovery_tests::parts(&dir, 8453, Address::ZERO).await;
    let ctx = &parts.ctx;
    let scope = Scope::Venue("base_dex".into());
    let p = Profile {
        chain: 8453, version: 6, book: Address::repeat_byte(1), book_hash:B256::repeat_byte(2),
        module: Address::repeat_byte(3), module_hash:B256::repeat_byte(4),
    };
    let fmt=format!("8453 6 {:#x} {:#x} {:#x} {:#x} rpc 123",p.book,p.book_hash,p.module,p.module_hash);
    for (key,val) in [
        ("dex.evm_chain_id",serde_json::json!(8453)), ("raindex.trading",serde_json::json!(true)),
        ("raindex.books",serde_json::json!([fmt])),
    ] {
        ctx.settings.set(key, scope.clone(), Some(val), "test").await.unwrap();
    }
    assert!(!ctx.flag("dex.history_scan").unwrap_or(false));
    assert!(ctx.int("dex.scan_progress_s").is_err());
    assert!(history::active(ctx,(p,123)));
    assert!(!history::active(ctx,(p,122)));
    let bsc = crate::venues::bsc_dex::hashflow::recovery_tests::parts(&dir,56,Address::ZERO).await;
    assert!(!history::active(&bsc.ctx,(p,123)));
    ctx.settings.set("raindex.trading",scope.clone(),Some(serde_json::json!(false)),"test").await.unwrap();
    assert!(!history::active(ctx,(p,123)));
    ctx.settings.set("raindex.trading",scope.clone(),Some(serde_json::json!(true)),"test").await.unwrap();
    ctx.settings.set("raindex.books",scope,Some(serde_json::json!([])),"test").await.unwrap();
    assert!(!history::active(ctx,(p,123)));
    drop(bsc);
    drop(parts);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn recent_base_removal_masks_old_rows_and_survives_reopening_while_history_lags() {
    let logs:Vec<Value>=serde_json::from_reader(GzDecoder::new(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/raindex_multiversion_history_events.json.gz"
    ).as_slice())).unwrap();
    let rem=logs.iter().find(|x| x["chain"]==8453 && x["version"]==6 && x["added"]==false).unwrap();
    let add=logs.iter().find(|x| x["book"]==rem["book"] && x["hash"]==rem["hash"] && x["added"]==true).unwrap();
    let p=Profile{ chain:8453,version:6,book:from_value(add["book"].clone()).unwrap(),book_hash:B256::ZERO,
        module:Address::ZERO,module_hash:B256::ZERO };
    let id:B256=from_value(rem["hash"].clone()).unwrap();
    let dir=crate::venues::bsc_dex::hashflow::recovery_tests::directory("raindex-recent",8453);
    let first=crate::venues::bsc_dex::hashflow::recovery_tests::parts(&dir,8453,Address::ZERO).await;
    let mut prev=None;
    let mut history=Indexed::default();
    let mut end=0;
    for r in [add,rem] {
        let mut l:Log=from_value(r["log"].clone()).unwrap();
        end=l.block_number.unwrap();
        let cursor=(end,l.block_hash.unwrap());
        let events=history_orders::changes(p,end,end,std::slice::from_mut(&mut l)).unwrap();
        let raw:Raw=events.into_iter().map(|(h,v)|(h,v.unwrap_or_default())).collect();
        let writes=raw.iter().map(|(&h,b)|(h,Some(b.clone()))).collect();
        assert!(history_store::commit(first.ctx.journal.db(),"base_dex","test:recent",
            prev,cursor,false,writes).await.unwrap());
        prev=Some(serde_json::json!(cursor).to_string());
        let recent=Indexed{cursor:Some(cursor),rows:Arc::new(raw)};
        let visible=View::for_book(p,1,end,&history).with_recent(end,history.cursor,&recent);
        assert_eq!(visible.iter().any(|(h,_)| *h==id),r["added"]==true);
        if r["added"]==true { history=recent; }
    }
    let snapshot=history.rows.clone();
    drop(first);
    let reopened=crate::venues::bsc_dex::hashflow::recovery_tests::parts(&dir,8453,Address::ZERO).await;
    let (cursor,rows)=history_store::load_recent(&reopened.ctx,"test:recent",6).await.unwrap().valid.unwrap();
    assert!(rows[&id].is_empty());
    let fresh=Indexed{cursor:Some(cursor),rows};
    assert_eq!(View::for_book(p,1,end,&history).with_recent(end,history.cursor,&fresh).iter().count(),0);
    assert!(snapshot.contains_key(&id)); // Копия полной истории не изменена свежим событием.
    assert!(history_store::load(&reopened.ctx,"test:recent",6).await.unwrap().valid.is_none());
    drop(reopened);
    std::fs::remove_dir_all(dir).unwrap();
}

