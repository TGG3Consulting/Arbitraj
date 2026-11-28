//! Адресный RPC replay свежей заявки и снятия при ещё далёком полном курсоре.
use super::*;
use crate::settings::scope::Scope;
use crate::venues::bsc_dex::hashflow::recovery_tests;
use axum::{Json, Router, routing::post};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU64, Ordering};

#[tokio::test]
async fn fresh_events_reach_the_reader_while_deployment_backfill_is_still_running() {
    let fixture: Vec<Value> = serde_json::from_reader(flate2::read::GzDecoder::new(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/raindex_multiversion_history_events.json.gz"
    ).as_slice())).unwrap();
    let remove = fixture.iter().find(|r| r["chain"]==8453 && r["version"]==6 && r["added"]==false).unwrap();
    let add = fixture.iter().find(|r| r["book"]==remove["book"] && r["hash"]==remove["hash"] && r["added"]==true).unwrap();
    let height = |r:&Value| u64::from_str_radix(r["blockNumber"].as_str().unwrap().trim_start_matches("0x"),16).unwrap();
    let events = [add["log"].clone(),remove["log"].clone()];
    let (first, last)=(height(&events[0]),height(&events[1]));
    let chain=remove["chain"].as_u64().unwrap();
    let id: B256 = serde_json::from_value(remove["hash"].clone()).unwrap();
    let book: Address = serde_json::from_value(remove["book"].clone()).unwrap();
    let tip = Arc::new(AtomicU64::new(first));
    let rpc_tip=tip.clone();
    let path = recovery_tests::directory("raindex-tip-replay",8453);
    let p = recovery_tests::parts(&path,8453,Address::ZERO).await;
    let scope=Scope::Venue("base_dex".into());
    let server=tokio::net::TcpListener::bind(("127.0.0.1",0)).await.unwrap();
    let endpoint=format!("http://{}/",server.local_addr().unwrap());
    let batch = last-first+1;
    let from = first-5*batch;
    let profile=Profile{ chain:8453,version:6,book,book_hash:B256::repeat_byte(1),
        module:Address::repeat_byte(2),module_hash:B256::repeat_byte(3) };
    let row=format!("8453 6 {book:#x} {:#x} {:#x} {:#x} rpc {from}",profile.book_hash,profile.module,profile.module_hash);
    for (k,s,v) in [
        ("dex.chain",scope.clone(),json!("base")), ("dex.evm_chain_id",scope.clone(),json!(8453)),
        ("bsc.node_http_url",scope.clone(),json!(endpoint)), ("bsc.node_http_rps",scope.clone(),json!(1000)),
        ("dex.logs_block_batch",scope.clone(),json!(batch)), ("http.timeout_ms",Scope::Global,json!(3000)),
        ("http.body_idle_ms",Scope::Global,json!(5000)), ("http.total_ms",Scope::Global,json!(8000)),
        ("http.max_body_bytes",Scope::Global,json!(2_000_000)),
        ("raindex.trading",scope.clone(),json!(true)), ("raindex.books",scope.clone(),json!([row])),
    ] {p.ctx.settings.set(k,s,Some(v),"test").await.unwrap();}
    assert!(active(&p.ctx,(profile,from)));
    assert!(p.ctx.flag("dex.history_scan").is_err() && p.ctx.int("dex.scan_progress_s").is_err());
    let handler=move|Json(q):Json<Value>|{
        let tip=rpc_tip.clone();let es=events.clone();
        async move {
            let num=|s:&str|u64::from_str_radix(s.trim_start_matches("0x"),16).unwrap();
            let result=match q["method"].as_str().unwrap() {
                "eth_chainId" => json!(format!("{chain:#x}")),
                "eth_getBlockByNumber" => {
                    let tag=q["params"][0].as_str().unwrap();
                    let n=if tag=="latest" {tip.load(Ordering::Relaxed)} else {num(tag)};
                    let hash=es.iter().find(|e|height(e)==n).map_or_else(||json!(B256::from(U256::from(n))),|e|e["blockHash"].clone());
                    json!({"number":format!("{n:#x}"),"hash":hash,"timestamp":"0x1"})
                }
                "eth_getLogs" => {
                    let f=&q["params"][0];assert_eq!(f["address"],json!(book));
                    let lo=num(f["fromBlock"].as_str().unwrap());let hi=num(f["toBlock"].as_str().unwrap());
                    assert!(hi>=lo && hi-lo<batch);
                    json!(es.iter().filter(|e|(lo..=hi).contains(&height(e))).collect::<Vec<_>>())
                }
                method=>panic!("unexpected archive method {method}"),
            };
            Json(json!({"jsonrpc":"2.0","id":q["id"],"result":result}))
        }
    };
    let serve=tokio::spawn(async move {axum::serve(server,Router::new().route("/",post(handler))).await.unwrap()});
    let (tx,full)=watch::channel(Indexed::default());
    let (head_tx,recent)=watch::channel(Indexed::default());
    let mut worker=Worker { profile,from,namespace:"test:full".into(),seed:None,tx,cursor:None,
        recent:newest::Recent::new(head_tx,"test:full") };
    for (at,available) in [(first,true),(last,false)] {
        tip.store(at,Ordering::Relaxed);
        worker.step(&p.ctx).await.unwrap();
        let scanned=full.borrow();
        assert!(scanned.cursor.unwrap().0 < first);
        assert_eq!(recent.borrow().cursor.unwrap().0,at);
        let view=View::for_book(profile,from,at,&scanned).with_recent(at,scanned.cursor,&recent.borrow());
        assert_eq!(view.iter().any(|(hash,_)| *hash==id),available);
    }
    serve.abort();drop(worker);drop(p);std::fs::remove_dir_all(path).unwrap();
}
