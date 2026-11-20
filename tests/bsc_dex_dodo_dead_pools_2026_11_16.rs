//! Файл: tests/bsc_dex_dodo_dead_pools_2026_11_16.rs
//! Что делает: 28 пулов DODO, которые бот не читает, на записанных ответах своего узла BSC (досъёмка 16.11.2026, блок
//! 133995628; tests/data/dodo_kjudge_2026-11-16_2, README там же).
//! Проверяется:
//! (1) по всем 328 пулам фабрик: откат KJUDGE_ERROR у getUserFeeRate(0x0) ⇔ K = 0 и нулевой резерв (21 из 21, ложных
//!   нет); у всех 21 обе котировки querySellBase / querySellQuote откатываются (KJUDGE_ERROR или TARGET_IS_ZERO) —
//!   торговать ими на цепи нельзя ни в одну сторону; 6 пулов «DPP Oracle 1.1.0» — откат ORACLE_NOT_FEASIBLE у
//!   getPMMStateForCall, резервы 0/0, обе котировки откатываются; 1 пул «DVM 1.0.0» — откат без данных у
//!   getUserFeeRate и обеих котировок; 28 = 21 + 6 + 1, и ни одного из них нет в /api/coins бота; в байткоде
//!   feeRateImpl 0xe5e9… строки «KJUDGE_ERROR» нет;
//! (2) НАШ код чтения (load_dodo.rs load) на тех же ответах (локальный узел отвечает записанным): рабочий пул
//!   читается (PoolState::Pmm); пул KJUDGE, пул с оракулом и DVM 1.0.0 — ошибка SwapReverted с текстом узла (откат
//!   вызова пула, а не сбой связи: pool_fail::is_link = false) — пул недостоверен, при решении 339 пауза растёт;
//! (3) журнал бота за сутки (539 записей) — один текст отката KJUDGE_ERROR.
//! Чего здесь нет: путь сессии пачки (listen / admit / reload::retry) — нужен узел WebSocket; он проверяется кодом
//! (DECISIONS «339 (журнал и приём, t153)»).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex (ctx, errors, pools::load_dodo, pools::pool_fail), arb::settings,
//! arb::storage, zstd, tokio (локальный узел).
//! Настройки: теста — узел (bsc.node_http_url = локальный), темп и тайм-ауты HTTP; паузы 3000 / 600000 мс — советы
//! docs/FIRST_RUN.md, здесь только как числа для расписания.

use std::collections::HashMap;
use std::time::Instant;

use alloy_primitives::{Address, U256};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::broadcast;

use arb::core::types::{PoolRef, PoolState};
use arb::gateway::errors::ErrorCode;
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::pools::load_dodo;
use arb::venues::bsc_dex::pools::pool_fail::{Fails, is_link};

const DIR: &str = "tests/data/dodo_kjudge_2026-11-16_2";
/// Блок записи H = 133995628.
const TAG: &str = "0x7fc9c6c";

/// Сжатая запись (zstd) → JSON. Ошибок нет (паника — записи нет).
fn rec_zst(name: &str) -> Value {
    let p = format!("{}/{DIR}/{name}", env!("CARGO_MANIFEST_DIR"));
    let raw = zstd::decode_all(std::fs::File::open(p).expect("запись")).expect("zstd");
    serde_json::from_slice(&raw).expect("JSON")
}

/// Первое 32-байтовое слово hex-ответа «0x…» → U256; не ответ (ошибка «ERR …») — None.
fn word(v: &Value) -> Option<U256> {
    let s = v.as_str()?.strip_prefix("0x")?;
    let b = alloy_primitives::hex::decode(s.get(..64)?).ok()?;
    Some(U256::from_be_slice(&b))
}

/// Строка ABI из ответа version(); не ответ — пусто.
fn version(v: &Value) -> String {
    let Some(b) = v
        .as_str()
        .and_then(|s| s.strip_prefix("0x"))
        .and_then(|s| alloy_primitives::hex::decode(s).ok())
    else {
        return String::new();
    };
    let len = U256::from_be_slice(&b[32..64]).to::<usize>();
    String::from_utf8(b[64..64 + len].to_vec()).unwrap_or_default()
}

/// Ответ — откат (строка «ERR execution reverted…»).
fn reverted(v: &Value) -> bool {
    v.as_str()
        .is_some_and(|s| s.starts_with("ERR execution reverted"))
}

/// Откат с данной причиной.
fn reverted_with(v: &Value, why: &str) -> bool {
    reverted(v) && v.as_str().is_some_and(|s| s.contains(why))
}

#[test]
fn dead_pools_facts() {
    let doc = rec_zst("pools_answers.json.zst");
    assert_eq!(doc["H"], 133_995_628);
    assert_eq!(doc["bytecode_e5e9_KJUDGE"].as_bool(), Some(false));
    let answers = doc["answers"].as_object().expect("answers");
    let pools = doc["pools"].as_array().expect("pools");
    assert_eq!((pools.len(), answers.len()), (328, 328));
    let (mut kjudge, mut oracle, mut other, mut in_api) = (0, 0, 0, 0);
    for p in pools {
        let addr = p["пул"].as_str().expect("пул");
        let a = &answers[addr];
        let api = p["в_API"]
            .as_array()
            .is_some_and(|x| x.last() == Some(&json!(true)));
        let k = word(&a["_K_()"]);
        let b = word(&a["_BASE_RESERVE_()"]);
        let q = word(&a["_QUOTE_RESERVE_()"]);
        let cond = k == Some(U256::ZERO) && (b == Some(U256::ZERO) || q == Some(U256::ZERO));
        let kj = reverted_with(&a["getUserFeeRate(0x0)"], "KJUDGE_ERROR");
        assert_eq!(cond, kj, "{addr}: KJUDGE ⇔ K = 0 и нулевой резерв");
        let both = reverted(&a["querySellBase(0x0,1)"]) && reverted(&a["querySellQuote(0x0,1)"]);
        if api {
            in_api += 1;
            assert!(!kj && !reverted(&a["getPMMStateForCall()"]), "{addr}");
            continue;
        }
        assert!(both, "{addr}: не в API — обе котировки откатываются");
        if kj {
            kjudge += 1;
            assert!(reverted_with(
                &a["feeImpl.getFeeRate(pool,0x0)"],
                "KJUDGE_ERROR"
            ));
        } else if reverted_with(&a["getPMMStateForCall()"], "ORACLE_NOT_FEASIBLE") {
            oracle += 1;
            assert_eq!(version(&a["version()"]), "DPP Oracle 1.1.0", "{addr}");
            assert_eq!((b, q), (Some(U256::ZERO), Some(U256::ZERO)), "{addr}");
        } else {
            other += 1;
            assert_eq!(version(&a["version()"]), "DVM 1.0.0", "{addr}");
            assert_eq!(
                a["getUserFeeRate(0x0)"], "ERR execution reverted: 0x",
                "{addr}"
            );
        }
    }
    assert_eq!((in_api, kjudge, oracle, other), (300, 21, 6, 1));
    // Байткод feeRateImpl из записи: строки KJUDGE_ERROR в нём нет (откат приходит из другого контракта).
    let raw = rec_zst("raw_rpc.json.zst");
    let code = raw
        .as_array()
        .expect("вызовы")
        .iter()
        .find(|x| x["method"] == "eth_getCode")
        .and_then(|x| x["result"].as_str())
        .expect("eth_getCode");
    assert!(!code.contains("4b4a554447455f4552524f52"));
}

/// Окружение на настоящей SQLite без настроек. Ошибок нет (паника — сбой среды теста).
async fn ctx() -> Ctx {
    let d = std::env::temp_dir().join(format!("arb-dodo-dead-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db, changes, journal.clone()).await.unwrap();
    {
        let c = Ctx {
            raw: RawLog::start(d.join("raw"), settings.clone()),
            pace: Default::default(),
            failover: Default::default(),
            place: arb::venues::bsc_dex::ctx::Place::BSC,
            chains: Default::default(),
            account: None,
            index: Default::default(),
            settings,
            vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
            http: reqwest::Client::new(),
            journal,
        };
        // Решение 416: тикер газа площадки DEX — настройка (dex.gas_ticker).
        c.settings
            .set(
                "dex.gas_ticker",
                arb::settings::scope::Scope::Venue("bsc_dex".into()),
                Some(serde_json::json!("BNB")),
                "тест",
            )
            .await
            .unwrap();
        c
    }
}

/// Записанные eth_call: (контракт, данные) → ответ JSON-RPC без id (result или error как есть).
fn recorded() -> HashMap<(String, String), Value> {
    rec_zst("raw_rpc.json.zst")
        .as_array()
        .expect("вызовы")
        .iter()
        .filter(|x| x["method"] == "eth_call")
        .map(|x| {
            let p = &x["params"][0];
            let key = (
                p["to"].as_str().unwrap_or_default().to_lowercase(),
                p["data"].as_str().unwrap_or_default().to_lowercase(),
            );
            let out = match x.get("error") {
                Some(e) => json!({"error": e}),
                None => json!({"result": x["result"]}),
            };
            (key, out)
        })
        .collect()
}

/// Ответ на запрос: записанный или ошибка «нет в записи». Ошибок нет.
fn answer(rec: &HashMap<(String, String), Value>, req: &Value) -> Value {
    let p = &req["params"][0];
    let to = p["to"].as_str().unwrap_or_default().to_lowercase();
    let data = p["input"]
        .as_str()
        .or(p["data"].as_str())
        .unwrap_or_default()
        .to_lowercase();
    let mut out = match (req["method"].as_str(), rec.get(&(to, data))) {
        (Some("eth_call"), Some(r)) => r.clone(),
        _ => json!({"error": {"code": -32000, "message": "нет в записи"}}),
    };
    out["jsonrpc"] = json!("2.0");
    out["id"] = req["id"].clone();
    out
}

/// Локальный узел: отвечает записанным (одиночный запрос и пачка JSON-RPC). Возвращает: адрес. Ошибок нет.
async fn node() -> String {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let rec = std::sync::Arc::new(recorded());
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            let rec = rec.clone();
            tokio::spawn(async move {
                let mut buf: Vec<u8> = Vec::new();
                let mut chunk = vec![0u8; 64 * 1024];
                // Заголовки, затем тело по content-length.
                let body = loop {
                    let k = s.read(&mut chunk).await.unwrap_or(0);
                    if k == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..k]);
                    let text = String::from_utf8_lossy(&buf).to_string();
                    let Some(at) = text.find("\r\n\r\n") else {
                        continue;
                    };
                    let len = text[..at]
                        .lines()
                        .find_map(|h| {
                            let (n, v) = h.split_once(':')?;
                            n.eq_ignore_ascii_case("content-length")
                                .then(|| v.trim().parse::<usize>().ok())?
                        })
                        .unwrap_or(0);
                    if buf.len() >= at + 4 + len {
                        break buf[at + 4..at + 4 + len].to_vec();
                    }
                };
                let v: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                let reply = match &v {
                    Value::Array(list) => {
                        Value::Array(list.iter().map(|r| answer(&rec, r)).collect())
                    }
                    one => answer(&rec, one),
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

#[tokio::test]
async fn our_dodo_read_on_recorded_answers() {
    let c = ctx().await;
    let url = node().await;
    let g = Scope::Global;
    for (k, v) in [
        ("bsc.node_http_url", json!(url)),
        ("bsc.node_http_rps", json!(100000)),
        ("http.timeout_ms", json!(5000)),
        ("http.body_idle_ms", json!(5000)),
        ("http.total_ms", json!(10000)),
        ("http.max_body_bytes", json!(10_000_000)),
    ] {
        c.settings.set(k, g.clone(), Some(v), "тест").await.unwrap();
    }
    let addr = |s: &str| s.parse::<Address>().expect("адрес");
    // Рабочий пул (есть в /api/coins бота) — читается.
    let ok = load_dodo::load(&c, addr("0x4ab9fb9487b8e5e17cba5664936019f11c064b2e"), TAG)
        .await
        .expect("рабочий пул читается");
    assert!(matches!(ok, PoolState::Pmm(_)));
    // Мёртвые: K = 0 и нулевой резерв (DPP, DSP, DVM), оракул без цены, DVM 1.0.0.
    let dead = [
        ("0x298ba9cd3acd572dce608ec682501c899628f176", "KJUDGE_ERROR"),
        ("0x2b1aa9b392e6b377485c8efa14a87a4779d9524e", "KJUDGE_ERROR"),
        ("0xb6f4cb89916dd529a5a6650e98269662f06f6d7d", "KJUDGE_ERROR"),
        (
            "0xfeafe253802b77456b4627f8c2306a9cebb5d681",
            "ORACLE_NOT_FEASIBLE",
        ),
        (
            "0x0ab58740a864d4fffdeffa39c24bf2845cde9a09",
            "execution reverted: 0x",
        ),
    ];
    let f = Fails::default();
    let t0 = Instant::now();
    for (pool, why) in dead {
        let e = load_dodo::load(&c, addr(pool), TAG)
            .await
            .expect_err("мёртвый пул не читается");
        assert_eq!(e.code, ErrorCode::SwapReverted, "{pool}: {}", e.message);
        assert!(e.message.contains(why), "{pool}: {}", e.message);
        assert!(!is_link(Some(e.code)), "{pool}: откат пула — не сбой узла");
        // Решение 339: неудачи пула удлиняют паузу (журнал — только первая).
        let r = PoolRef::Contract(addr(pool));
        let why = (Some(e.code), e.message.as_str());
        let (a, fresh) = f.record(&r, why, 3_000, 600_000, t0);
        let (b, again) = f.record(&r, why, 3_000, 600_000, t0);
        assert!(fresh && !again, "{pool}");
        assert_eq!((a.pause_ms, b.pause_ms), (3_000, 6_000), "{pool}");
    }
}

#[test]
fn journal_kjudge_records_24h() {
    let p = format!(
        "{}/{DIR}/kjudge_journal_entries_24h.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let v: Value =
        serde_json::from_str(&std::fs::read_to_string(p).expect("запись")).expect("JSON");
    let list = v.as_array().expect("массив");
    assert_eq!(list.len(), 539);
    for x in list {
        assert_eq!(x["kind"], "dex.pool");
        let m = x["message"].as_str().expect("message");
        assert!(m.contains("код 3: execution reverted: KJUDGE_ERROR"), "{m}");
    }
}
