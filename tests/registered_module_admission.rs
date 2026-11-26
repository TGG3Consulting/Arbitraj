//! Проверяет штатный допуск в Rust. Узел отдаёт записанный runtime и factory() той же сборки.
//! Не подключается к публичному RPC, все изменения настроек относятся к временной SQLite.
mod route_common;
use alloy_primitives::{Address, B256};
use arb::{
    settings::scope::Scope,
    venues::bsc_dex::pools::{direct_module, families},
};
use axum::{Json, Router, routing::post};
use serde_json::{Value, json};

async fn rpc(factory: Address, runtime: &str, addr: Address) -> String {
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", socket.local_addr().unwrap());
    let runtime = runtime.to_string();
    let respond = move |Json(body): Json<Value>| {
        let runtime = runtime.clone();
        async move {
            let frame = |q: &Value| {
                let params = &q["params"];
                let pinned = json!({"blockHash":B256::repeat_byte(1),"requireCanonical":true});
                let target = json!(addr);
                let result = match q["method"].as_str().unwrap() {
                    "eth_chainId" => json!("0x2105"),
                    "eth_getBlockByNumber" => json!({"hash": B256::repeat_byte(1)}),
                    "eth_getCode" if params[0] == target && params[1] == pinned => json!(runtime),
                    "eth_call"
                        if params[0]["to"] == target
                            && params[0]["data"] == "0xc45a0155"
                            && params[1] == pinned =>
                    {
                        json!(factory.into_word())
                    }
                    _ => {
                        return json!({"jsonrpc":"2.0","id":q["id"],"error":{"code":-32601,"message":"unexpected read"}});
                    }
                };
                json!({"jsonrpc":"2.0","id":q["id"],"result":result})
            };
            Json(match body.as_array() {
                Some(items) => Value::Array(items.iter().map(frame).collect()),
                None => frame(&body),
            })
        }
    };
    let app = Router::new().route("/", post(respond));
    tokio::spawn(async move {
        axum::serve(socket, app).await.unwrap();
    });
    url
}

#[tokio::test]
async fn live_settings_and_compiled_factory_binding_agree_before_execution() {
    let mut ctx = route_common::ctx().await;
    ctx.place = arb::venues::bsc_dex::ctx::Place {
        id: "base_dex",
        all: &["bsc_dex", "base_dex"],
    };
    let v = Scope::Venue(ctx.place.id.to_string());
    let set = async |key: &str, scope: Scope, value: Value| {
        ctx.settings
            .set(key, scope, Some(value), "тест")
            .await
            .unwrap();
    };
    for (key, value) in [
        ("http.timeout_ms", 5000),
        ("http.body_idle_ms", 5000),
        ("http.total_ms", 10000),
        ("http.max_body_bytes", 1_000_000),
    ] {
        set(key, Scope::Global, json!(value)).await;
    }
    set("dex.chain", v.clone(), json!("base")).await;
    set("dex.evm_chain_id", v.clone(), json!(8453)).await;
    set("dex.gas_mode", v.clone(), json!("op_stack_eip1559")).await;
    set("bsc.node_http_rps", v.clone(), json!(100)).await;
    let archive: Value = serde_json::from_reader(flate2::read::GzDecoder::new(
        &include_bytes!("data/first_run_dex_2026-11-26/registered_modules_runtime.json.gz")[..],
    ))
    .unwrap();
    let address = Address::repeat_byte(0x73);
    for (id, data) in archive.as_object().unwrap() {
        let scope = Scope::Dex(id.clone());
        let factory = data["factory"].as_str().unwrap().parse().unwrap();
        let active = data["kind"].as_str().unwrap();
        let expected = families::make(id, active).unwrap();
        let url = rpc(factory, data["runtime"].as_str().unwrap(), address).await;
        set("bsc.node_http_url", v.clone(), json!(url)).await;
        set("dex.kind", scope.clone(), json!(active)).await;
        set("dex.router", scope.clone(), json!(address)).await;
        set("dex.factory", scope.clone(), json!(factory)).await;
        set(
            "dex.module_runtime_hashes",
            scope.clone(),
            json!([data["hash"]]),
        )
        .await;
        direct_module::verify(&ctx, expected, address, "latest")
            .await
            .unwrap();
        let at = json!({"blockHash":B256::repeat_byte(1),"requireCanonical":true});
        direct_module::verify_factory(&ctx, expected, address, &at)
            .await
            .unwrap();
        assert!(
            direct_module::verify_factory(&ctx, expected, address, &json!("latest"))
                .await
                .is_err()
        );
        set(
            "dex.factory",
            scope.clone(),
            json!(Address::repeat_byte(0x74)),
        )
        .await;
        assert!(
            direct_module::verify(&ctx, expected, address, "latest")
                .await
                .is_err()
        );
        set("dex.factory", scope.clone(), json!(factory)).await;
        let other = if active == "memebox_v5" {
            "registered_v3_module"
        } else {
            "memebox_v5"
        };
        set("dex.kind", scope.clone(), json!(other)).await;
        assert!(
            direct_module::verify(&ctx, expected, address, "latest")
                .await
                .is_err()
        );
        set("dex.kind", scope.clone(), json!(active)).await;
        set(
            "dex.module_runtime_hashes",
            scope,
            json!([B256::repeat_byte(0x75)]),
        )
        .await;
        assert!(
            direct_module::verify(&ctx, expected, address, "latest")
                .await
                .is_err()
        );
    }
}
