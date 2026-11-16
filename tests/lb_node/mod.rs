//! Файл: tests/lb_node/mod.rs
//! Что делает: ревью решения 332 — локальный узел JSON-RPC по HTTP (адрес-петля, как tests/bsc_dex_tip_gas_plan.rs),
//! который отвечает на вызовы пула Liquidity Book V2.1 ТОЛЬКО записанными данными tests/data/lb_v21_2026-11-15
//! (probe.json — блок 133716961, probe_time.json — блок 133732452): eth_call getActiveId, getStaticFeeParameters,
//! getVariableFeeParameters, getTokenX, getBinStep, getBin, getNextNonEmptyBin (одиночные и пачкой JSON-RPC) и
//! eth_getBlockByNumber (поле timestamp — время блока записи; mixHash не записан и не отдаётся — поле необязательное,
//! node/head_age.rs). Ответ выбирается по блоку запроса и адресу пула; вызова или блока нет в записи — ошибка JSON-RPC.
//! getBin(id) непустого бина — записанные резервы; бина нет среди записанных — (0, 0): запись содержит ВСЕ непустые бины
//! пула (обход getNextNonEmptyBin до края в обе стороны, README записи). getNextNonEmptyBin — ближайший записанный
//! непустой бин строго ниже (swapForY = true) / строго выше; нет — type(uint24).max / 0 (TreeMath.findFirstRight /
//! findFirstLeft — LBPair.sol L193-L195, TreeMath.sol L147, L202; так записанный обход и шёл). Логики площадки нет — это
//! выдача записанного, чтобы настоящий код бота (load_lb.rs, load_lb_bins.rs, lb_head.rs) шёл к «узлу» как в работе.
//! Кто вызывает: tests/bsc_dex_off_map_bins_332.rs, tests/bsc_dex_lb_head_332.rs.
//! От чего зависит: arb::venues::bsc_dex::{pools::abi_lb, ctx}, arb::settings, tokio, serde_json.
//! Настройки (задаёт attach): bsc.node_http_url (петля), bsc.node_http_rps, http.timeout_ms, http.body_idle_ms,
//! http.total_ms, http.max_body_bytes.
#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, U256};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use arb::settings::scope::Scope;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::pools::abi_lb::ILBPair as P;

/// Папка записи.
const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/lb_v21_2026-11-15/");
/// type(uint24).max — ответ «ниже бинов нет» (TreeMath.sol L147).
const NONE_BELOW: u32 = 0xff_ffff;

/// Записанное состояние пулов на одном блоке.
pub struct Rec {
    /// Номер блока записи.
    pub block: u64,
    /// Время блока записи (секунды Unix).
    pub ts: u64,
    /// Пулы по адресу: запись пула (поля как в файле).
    pub pools: HashMap<Address, Value>,
}

/// Файл записи как JSON. Паника — файла нет (тест).
pub fn json_of(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(format!("{DIR}{name}")).unwrap()).unwrap()
}

/// Адрес из строки записи.
pub fn addr(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}

/// Запись одного файла (probe.json или probe_time.json).
pub fn rec(name: &str) -> Rec {
    let v = json_of(name);
    let pools = v["pools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (addr(&p["pool"]), p.clone()))
        .collect();
    Rec {
        block: v["block"].as_u64().unwrap(),
        ts: v["timestamp"].as_u64().unwrap(),
        pools,
    }
}

/// Записанные непустые бины пула: id → (X, Y).
pub fn bins_of(p: &Value) -> BTreeMap<u32, (u128, u128)> {
    p["bins"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| {
            let n = |i: usize| v[i].as_str().unwrap().parse::<u128>().unwrap();
            (k.parse::<u32>().unwrap(), (n(0), n(1)))
        })
        .collect()
}

/// Число из записанной строки → слово ABI.
fn word(v: &Value) -> [u8; 32] {
    U256::from_str_radix(v.as_str().unwrap(), 10)
        .unwrap()
        .to_be_bytes::<32>()
}

/// Слова подряд → hex ответа eth_call.
fn hex(words: &[[u8; 32]]) -> String {
    let mut b = Vec::new();
    for w in words {
        b.extend_from_slice(w);
    }
    alloy_primitives::hex::encode_prefixed(b)
}

/// Ответ eth_call по записи. Принимает: записи, параметры запроса. Возвращает: hex результата или текст ошибки.
fn eth_call(recs: &[Rec], params: &Value) -> Result<String, String> {
    let tag = params[1].as_str().unwrap_or_default();
    let r = recs
        .iter()
        .find(|r| format!("{:#x}", r.block) == tag)
        .ok_or_else(|| format!("блока {tag} нет в записи"))?;
    let to = addr(&params[0]["to"]);
    let p = r
        .pools
        .get(&to)
        .ok_or_else(|| format!("пула {to} нет в записи"))?;
    let input = alloy_primitives::hex::decode(params[0]["input"].as_str().unwrap()).unwrap();
    let sel: [u8; 4] = input[..4].try_into().unwrap();
    let list = |k: &str| -> Vec<[u8; 32]> { p[k].as_array().unwrap().iter().map(word).collect() };
    if sel == P::getActiveIdCall::SELECTOR {
        return Ok(hex(&[word(&p["activeId"])]));
    }
    if sel == P::getStaticFeeParametersCall::SELECTOR {
        return Ok(hex(&list("static")));
    }
    if sel == P::getVariableFeeParametersCall::SELECTOR {
        return Ok(hex(&list("variable")));
    }
    if sel == P::getBinStepCall::SELECTOR {
        return Ok(hex(&[word(&p["binStep"])]));
    }
    if sel == P::getTokenXCall::SELECTOR {
        let mut w = [0u8; 32];
        w[12..].copy_from_slice(addr(&p["tokenX"]).as_slice());
        return Ok(hex(&[w]));
    }
    let bins = bins_of(p);
    if sel == P::getBinCall::SELECTOR {
        let c = P::getBinCall::abi_decode(&input).unwrap();
        let (x, y) = bins.get(&c.id.to::<u32>()).copied().unwrap_or((0, 0));
        let w = |v: u128| U256::from(v).to_be_bytes::<32>();
        return Ok(hex(&[w(x), w(y)]));
    }
    if sel == P::getNextNonEmptyBinCall::SELECTOR {
        let c = P::getNextNonEmptyBinCall::abi_decode(&input).unwrap();
        let id: u32 = c.id.to();
        let next = if c.swapForY {
            bins.range(..id).next_back().map_or(NONE_BELOW, |(k, _)| *k)
        } else {
            bins.range(id + 1..).next().map_or(0, |(k, _)| *k)
        };
        return Ok(hex(&[U256::from(next).to_be_bytes::<32>()]));
    }
    Err(format!(
        "вызова {} нет в записи",
        alloy_primitives::hex::encode(sel)
    ))
}

/// Ответ на один запрос JSON-RPC.
fn answer(recs: &[Rec], req: &Value) -> Value {
    let id = req["id"].clone();
    let res = match req["method"].as_str() {
        Some("eth_call") => eth_call(recs, &req["params"]).map(Value::String),
        Some("eth_getBlockByNumber") => {
            let tag = req["params"][0].as_str().unwrap_or_default();
            recs.iter()
                .find(|r| format!("{:#x}", r.block) == tag)
                .map(|r| json!({ "number": tag, "timestamp": format!("{:#x}", r.ts) }))
                .ok_or_else(|| format!("блока {tag} нет в записи"))
        }
        other => Err(format!("метода {other:?} нет в записи")),
    };
    match res {
        Ok(v) => json!({ "jsonrpc": "2.0", "id": id, "result": v }),
        Err(m) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32000, "message": m } }),
    }
}

/// Тело HTTP-запроса целиком (по content-length).
async fn body(s: &mut tokio::net::TcpStream) -> String {
    let mut buf = Vec::new();
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let k = s.read(&mut chunk).await.unwrap_or(0);
        if k == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..k]);
        let text = String::from_utf8_lossy(&buf).to_string();
        if let Some(at) = text.find("\r\n\r\n") {
            let len = text[..at]
                .lines()
                .find_map(|l| {
                    let l = l.to_ascii_lowercase();
                    l.strip_prefix("content-length:")
                        .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                })
                .unwrap_or(0);
            if buf.len() >= at + 4 + len {
                return String::from_utf8_lossy(&buf[at + 4..at + 4 + len]).to_string();
            }
        }
    }
    String::new()
}

/// Запускает узел на записях. Возвращает: адрес http://127.0.0.1:порт.
pub async fn node(recs: Vec<Rec>) -> String {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let recs = Arc::new(recs);
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            let recs = recs.clone();
            tokio::spawn(async move {
                let req: Value = serde_json::from_str(&body(&mut s).await).unwrap_or(Value::Null);
                let reply = match &req {
                    Value::Array(list) => {
                        Value::Array(list.iter().map(|r| answer(&recs, r)).collect())
                    }
                    one => answer(&recs, one),
                }
                .to_string();
                let r = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
                    reply.len()
                );
                let _ = s.write_all(r.as_bytes()).await;
            });
        }
    });
    url
}

/// Настройки узла в окружении теста (вся система). Принимает: окружение, адрес узла.
pub async fn attach(c: &Ctx, url: &str) {
    for (k, v) in [
        ("bsc.node_http_url", json!(url)),
        ("bsc.node_http_rps", json!(1000)),
        ("http.timeout_ms", json!(5000)),
        ("http.body_idle_ms", json!(5000)),
        ("http.total_ms", json!(10000)),
        ("http.max_body_bytes", json!(1_000_000)),
    ] {
        c.settings
            .set(k, Scope::Global, Some(v), "тест")
            .await
            .unwrap();
    }
}
