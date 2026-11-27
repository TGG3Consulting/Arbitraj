//! Узел только для чтения отдаёт сохранённую подписанную транзакцию в каноническом блоке или в заменившем его блоке.
use super::{Archive, Parts};
use crate::settings::scope::Scope;
use alloy_primitives::B256;
use axum::{Json, Router, routing::post};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
pub(super) async fn start(
    p: &Parts,
    row: &Archive,
) -> (Arc<AtomicBool>, tokio::task::JoinHandle<()>) {
    let socket = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let endpoint = format!("http://{}", socket.local_addr().unwrap());
    let archive: Vec<Value> = serde_json::from_str(include_str!(
        "../../../../tests/data/hashflow_direct_2026-11-26/swaps.json"
    ))
    .unwrap();
    let row = archive
        .into_iter()
        .find(|v| v["transaction_hash"] == json!(row.transaction_hash))
        .unwrap();
    let chain = row["chain_id"].as_u64().unwrap();
    let canonical = Arc::new(AtomicBool::new(true));
    let switch = canonical.clone();
    let handler = move |Json(q): Json<Value>| {
        let row = row.clone();
        let switch = switch.clone();
        async move {
            let hash = row["transaction_hash"].clone();
            let block = row["block_hash"].clone();
            let receipt = &row["receipt"];
            let index = receipt["transactionIndex"].clone();
            let height = receipt["blockNumber"].clone();
            let n =
                u64::from_str_radix(height.as_str().unwrap().trim_start_matches("0x"), 16).unwrap();
            let result = match q["method"].as_str().unwrap() {
                "eth_chainId" => json!(format!("0x{chain:x}")),
                "eth_blockNumber" => json!(format!("0x{:x}", n + 10)),
                "eth_getTransactionByHash" => {
                    json!({"hash":hash,"chainId":format!("0x{chain:x}"),"from":row["from"],"to":row["to"],
                    "nonce":format!("0x{:x}",row["nonce"].as_u64().unwrap()),"value":row["value"],"input":row["input"],"blockHash":block,"transactionIndex":index})
                }
                "eth_getTransactionReceipt" => {
                    let mut r = receipt.clone();
                    r["transactionHash"] = hash;
                    r["blockHash"] = block;
                    r["l1Fee"] = json!("0x0");
                    r
                }
                "eth_getBlockByNumber" => {
                    let at =
                        usize::from_str_radix(index.as_str().unwrap().trim_start_matches("0x"), 16)
                            .unwrap();
                    let mut hashes = vec![json!(B256::ZERO); at + 1];
                    hashes[at] = hash;
                    json!({"number":height,"hash":if switch.load(Ordering::SeqCst){block}else{json!(B256::ZERO)},"transactions":hashes})
                }
                other => panic!("unexpected RPC {other}"),
            };
            Json(json!({"jsonrpc":"2.0","id":q["id"],"result":result}))
        }
    };
    let srv = Router::new().route("/", post(handler));
    let task = tokio::spawn(async move {
        axum::serve(socket, srv).await.unwrap();
    });
    for (key, scope, value) in [
        ("http.timeout_ms", Scope::Global, json!(5000)),
        ("http.body_idle_ms", Scope::Global, json!(5000)),
        ("http.total_ms", Scope::Global, json!(8000)),
        ("http.max_body_bytes", Scope::Global, json!(2_000_000)),
        (
            "dex.evm_chain_id",
            Scope::Venue(p.ctx.place.id.into()),
            json!(chain),
        ),
        (
            "bsc.node_http_url",
            Scope::Venue(p.ctx.place.id.into()),
            json!(endpoint),
        ),
        (
            "bsc.node_http_rps",
            Scope::Venue(p.ctx.place.id.into()),
            json!(1000),
        ),
        (
            "send.deposit_confirmations",
            Scope::Venue(p.ctx.place.id.into()),
            json!(1),
        ),
    ] {
        p.ctx
            .settings
            .set(key, scope, Some(value), "test")
            .await
            .unwrap();
    }
    (canonical, task)
}
