//! Файл: tests/bsc_dex_review_t153.rs
//! Что делает: ревью ветки t153 (решения 339, 330, 331; DECISIONS «339 (ревью, t153)»). Проверяется:
//! - Н9 «не задано = 1 в 1»: на настоящей SQLite без настроек решения 339 / 330 / 331 / t160 выключены (base, alert_after,
//!   limit — None; dex.per_block_skip_same не задана), отметок нет, pool_fail::requeue без изоляции = stale_read::requeue;
//! - Н9 одновременное чтение (load_batch_par::states) = прежнее (load_batch::states) на ЗАПИСАННЫХ ответах узла BSC:
//!   блок 0x7b936ae, bsc-dataseed.bnbchain.org, 2026-10-25 (tests/data/indexers_2026-10-24/arb_node_responses_3_2026-10-25.json,
//!   пулы CAKE и ETH PancakeSwap V2/V3, Uniswap V3, PancakeSwap Infinity CL, Uniswap V4 — как tests/bsc_dex_batch_load.rs)
//!   плюс пул вида «по одному» (DODO DVM) на месте 1 — проверка раскладки индексов (solo / rest). Узел — локальный
//!   JSON-RPC по HTTP (адрес-петля, как tests/bsc_dex_graph_noroute_gas.rs): отвечает ЗАПИСАННЫМ ответом на тот же
//!   eth_call (контракт, данные), на незаписанный — ошибкой JSON-RPC (у DODO записи нет — у обоих путей та же причина);
//! - Б1 (requeue): при изоляции пул, брошенный из-за незаданной настройки, остаётся брошенным; иначе — в очередь повтора;
//! - Н4: ошибка связи / лимита узла пауз не растит; Н3: «сейчас + пауза» без паники; Н5 / Н6: счётчик и чистка отметок;
//!   Н7: уведомление один раз на эпизод; узел — одна запись на сбой;
//! - EventLanes без узла: finish (итог true → очередь повтора, ошибка → End), shutdown;
//! - каталог: новые настройки необязательные, только площадка DEX; событие dex.pool_unread с заголовком.
//!
//! Чего здесь нет (нужна запись с живого бота, docs/TASKS.md t153): путь сессии с ответом KJUDGE_ERROR.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{ctx, pools::{pool_fail, load_batch, load_batch_par, session_lanes, …}},
//! arb::settings, arb::storage, arb::notify, tokio (локальный узел).
//! Настройки: теста — значения записи (dex.tick_words = 2, dex.load_batch = 200, dex.multicall3, TickLens, StateView).

use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::time::{Duration, Instant};

use alloy_primitives::{Address, B256};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc};

use arb::core::types::PoolRef;
use arb::gateway::errors::ErrorCode;
use arb::settings::catalog;
use arb::settings::schema::VenueFor;
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::pools::admit::Members;
use arb::venues::bsc_dex::pools::group::Msg;
use arb::venues::bsc_dex::pools::listen::End;
use arb::venues::bsc_dex::pools::load::Loaded;
use arb::venues::bsc_dex::pools::load_batch::{self, Miss, Want};
use arb::venues::bsc_dex::pools::pool_fail::{self, Fails, later};
use arb::venues::bsc_dex::pools::registry::Registry;
use arb::venues::bsc_dex::pools::reload::{Missing, SKIP_SAME};
use arb::venues::bsc_dex::pools::revive::Abandoned;
use arb::venues::bsc_dex::pools::session_lanes::EventLanes;
use arb::venues::bsc_dex::pools::{families, load_batch_par, stale_read};
use arb::venues::bsc_dex::tokens::Tokens;

const REC: &str = "tests/data/indexers_2026-10-24/arb_node_responses_3_2026-10-25.json";
const POOLS: &str = "tests/data/node_calls_3_2026-10-24.json";

/// Окружение на настоящей SQLite без настроек. Принимает: метку папки. Ошибок нет (паника — сбой среды теста).
async fn ctx(tag: &str) -> Ctx {
    let d = std::env::temp_dir().join(format!("arb-review-t153-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db, changes, journal.clone()).await.unwrap();
    Ctx {
        raw: RawLog::start(d.join("raw"), settings.clone()),
        pace: Default::default(),
        failover: Default::default(),
        account: None,
        index: Default::default(),
        settings,
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        http: reqwest::Client::new(),
        journal,
    }
}

/// Записанные eth_call: (контракт, данные запроса) → результат (строка 0x…). Ошибок нет.
fn recorded() -> HashMap<(String, String), String> {
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(REC).unwrap()).unwrap();
    doc["results"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["request"]["method"] == "eth_call")
        .map(|r| {
            let p = &r["request"]["params"][0];
            let key = (
                p["to"].as_str().unwrap().to_lowercase(),
                p["input"].as_str().unwrap().to_lowercase(),
            );
            (key, r["response"]["result"].as_str().unwrap().to_string())
        })
        .collect()
}

/// Ответ на один запрос JSON-RPC: записанный результат eth_call или ошибка «нет в записи». Ошибок нет.
fn answer(rec: &HashMap<(String, String), String>, req: &Value) -> Value {
    let p = &req["params"][0];
    let to = p["to"].as_str().unwrap_or_default().to_lowercase();
    let data = p["input"]
        .as_str()
        .or(p["data"].as_str())
        .unwrap_or_default()
        .to_lowercase();
    match (req["method"].as_str(), rec.get(&(to, data))) {
        (Some("eth_call"), Some(r)) => json!({"jsonrpc": "2.0", "id": req["id"], "result": r}),
        _ => {
            json!({"jsonrpc": "2.0", "id": req["id"], "error": {"code": -32000, "message": "нет в записи"}})
        }
    }
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

/// Пулы записи (как tests/bsc_dex_batch_load.rs pools): (вид, ссылка, tickSpacing). Ошибок нет.
fn pools() -> Vec<(&'static families::Family, PoolRef, Option<i32>)> {
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(POOLS).unwrap()).unwrap();
    let cl: Address = "0xa0ffb9c1ce1fe56963b0321b32e7a0302114058b"
        .parse()
        .unwrap();
    let v4: Address = "0x28e2ea090877bf75740558f6bfb36a5ffee9e9df"
        .parse()
        .unwrap();
    doc["pools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            let tag = p["tag"].as_str().unwrap();
            let dex = tag.split(' ').nth(1).unwrap();
            let (kind, manager) = match p["kind"].as_str().unwrap() {
                "v2" => ("pancakeswap_v2", None),
                "pcs_v3" => ("pancakeswap_v3", None),
                "uni_v3" => ("uniswap_v3", None),
                "infinity_cl" => ("pancakeswap_infinity_cl", Some(cl)),
                _ => ("uniswap_v4", Some(v4)),
            };
            let f = families::make(dex, kind).unwrap();
            let reference = match manager {
                None => PoolRef::Contract(p["addr"].as_str().unwrap().parse().unwrap()),
                Some(manager) => PoolRef::Singleton {
                    manager,
                    id: p["id"].as_str().unwrap().parse::<B256>().unwrap(),
                },
            };
            let sp = p["spacing"].as_i64().unwrap() as i32;
            (f, reference, (sp > 0).then_some(sp))
        })
        .collect()
}

/// Итог чтения пула для сравнения: состояние, тики, бины, комиссия — или код и текст причины. Ошибок нет.
fn same(a: &Result<Loaded, Miss>, b: &Result<Loaded, Miss>) -> bool {
    match (a, b) {
        (Ok(x), Ok(y)) => {
            x.state == y.state && x.ticks == y.ticks && x.bins == y.bins && x.fee_pips == y.fee_pips
        }
        (Err(x), Err(y)) => x.code == y.code && x.message == y.message,
        _ => false,
    }
}

#[tokio::test]
async fn unset_is_as_before() {
    let c = ctx("unset").await;
    assert_eq!(
        pool_fail::base(&c),
        None,
        "dex.pool_fail_pause_ms не задана — изоляции нет"
    );
    assert_eq!(
        pool_fail::alert_after(&c),
        None,
        "dex.pool_unread_alert_s не задана — уведомления нет"
    );
    for n in [
        "dex.event_lanes_max",
        "dex.block_read_parallel_max",
        "verify.read_parallel_max",
    ] {
        assert_eq!(
            load_batch_par::limit(&c, n),
            None,
            "{n} не задана — по очереди"
        );
    }
    assert!(
        c.flag(SKIP_SAME).is_err(),
        "dex.per_block_skip_same не задана"
    );
    let f = Fails::default();
    let r = PoolRef::Contract(Address::ZERO);
    assert!(
        !f.any() && f.due(&r, Instant::now()),
        "без 339 отметок нет — читать можно всегда"
    );
    // Без изоляции requeue = stale_read::requeue (как до ветки): брошенный из-за настройки — снова в очередь.
    let ab = || {
        vec![Abandoned {
            i: 2,
            setting: Some("dex.load_batch".into()),
        }]
    };
    let (mut m1, mut a1) = (Vec::<Missing>::new(), ab());
    let (mut m2, mut a2) = (Vec::<Missing>::new(), ab());
    pool_fail::requeue(false, &mut m1, &mut a1, 2);
    stale_read::requeue(&mut m2, &mut a2, 2);
    assert_eq!(
        m1.iter().map(|m| (m.i, m.tries)).collect::<Vec<_>>(),
        vec![(2, 0)]
    );
    assert_eq!(
        m2.iter().map(|m| (m.i, m.tries)).collect::<Vec<_>>(),
        vec![(2, 0)]
    );
    assert!(a1.is_empty() && a2.is_empty());
}

#[tokio::test]
async fn parallel_read_equals_sequential_on_recorded_answers() {
    let c = ctx("par").await;
    let url = node().await;
    let v = Scope::Venue("bsc_dex".into());
    let g = Scope::Global;
    let list = [
        ("bsc.node_http_url", g.clone(), json!(url)),
        ("bsc.node_http_rps", g.clone(), json!(100000)),
        ("http.timeout_ms", g.clone(), json!(5000)),
        ("http.body_idle_ms", g.clone(), json!(5000)),
        ("http.total_ms", g.clone(), json!(10000)),
        ("http.max_body_bytes", g, json!(10_000_000)),
        ("dex.tick_words", v.clone(), json!(2)),
        ("dex.load_batch", v.clone(), json!(200)),
        (
            "dex.multicall3",
            v,
            json!("0xca11bde05977b3631167028862be2a173976ca11"),
        ),
        (
            "dex.tick_lens",
            Scope::Dex("pancakeswap-v3-bsc".into()),
            json!("0x9a489505a00cE272eAa5e07Dba6491314CaE3796"),
        ),
        (
            "dex.tick_lens",
            Scope::Dex("uniswap-bsc".into()),
            json!("0xD9270014D396281579760619CCf4c3af0501A47C"),
        ),
        (
            "dex.state_view",
            Scope::Dex("uniswap-v4-bsc".into()),
            json!("0xd13dd3d6e93f276fafc9db9e6bb47c1180aee0c4"),
        ),
    ];
    for (n, s, val) in list {
        c.settings.set(n, s, Some(val), "тест").await.unwrap();
    }
    let mut ps = pools();
    let n_rec = ps.len();
    // Пул вида «по одному» на месте 1 (DODO DVM, адрес первого пула записи): ответов на его вызовы в записи нет.
    let dodo = families::make("dodo-test", "dodo_dvm").unwrap();
    assert!(load_batch::one_by_one(dodo));
    let at = ps[0].1.clone();
    ps.insert(1, (dodo, at, None));
    let wants: Vec<Want> = ps
        .iter()
        .map(|(f, r, sp)| Want {
            family: f,
            reference: r,
            spacing: *sp,
            step: None,
            usdt: Address::ZERO,
            token0: Address::ZERO,
            token1: Address::ZERO,
        })
        .collect();
    let (tokens, reg) = (Tokens::default(), Registry::default());
    let block = u64::from_str_radix("7b936ae", 16).unwrap();
    let before = load_batch_par::states_with(&c, &tokens, &reg, &wants, block, None).await;
    for par in [1usize, 2, 8] {
        let now = load_batch_par::states_with(&c, &tokens, &reg, &wants, block, Some(par)).await;
        assert_eq!(now.len(), before.len());
        for (i, (a, b)) in before.iter().zip(&now).enumerate() {
            assert!(
                same(a, b),
                "пул {i} ({:?}): одновременно (предел {par}) ≠ по очереди",
                wants[i].reference
            );
        }
    }
    let ok = before.iter().filter(|r| r.is_ok()).count();
    assert_eq!(
        ok,
        n_rec,
        "все пулы записи прочитаны по записанным ответам: {:?}",
        before
            .iter()
            .map(|r| r.as_ref().err().map(|e| e.message.clone()))
            .collect::<Vec<_>>()
    );
    assert!(
        before[1].is_err(),
        "у DODO записи нет — причина у обоих путей одна"
    );
}

#[test]
fn link_errors_do_not_grow_pause_and_log_once() {
    let f = Fails::default();
    let r =
        PoolRef::Contract(Address::from_str("0x0000000000000000000000000000000000000001").unwrap());
    let t0 = Instant::now();
    let node = (Some(ErrorCode::Timeout), "узел не ответил");
    let (x, _) = f.record(&r, node, 1_000, 60_000, t0);
    assert_eq!((x.tries, x.grow, x.pause_ms), (1, 0, 1_000));
    let (x, fresh) = f.record(
        &r,
        (Some(ErrorCode::Timeout), "узел не ответил за 7001 мс"),
        1_000,
        60_000,
        t0,
    );
    assert!(!fresh, "тот же код — не новая причина");
    assert_eq!(
        (x.tries, x.grow, x.pause_ms),
        (2, 0, 1_000),
        "ошибка узла пауз не растит"
    );
    let (x, _) = f.record(
        &r,
        (Some(ErrorCode::SwapReverted), "execution reverted"),
        1_000,
        60_000,
        t0,
    );
    assert_eq!((x.grow, x.pause_ms), (1, 1_000));
    let (x, _) = f.record(
        &r,
        (Some(ErrorCode::SwapReverted), "execution reverted"),
        1_000,
        60_000,
        t0,
    );
    assert_eq!(
        (x.grow, x.pause_ms),
        (2, 2_000),
        "ошибка пула — пауза растёт"
    );
    let (x, _) = f.record(&r, (Some(ErrorCode::RateLimited), "429"), 1_000, 60_000, t0);
    assert_eq!(
        x.pause_ms, 1_000,
        "лимит узла — начальная пауза, не потолок"
    );
    assert!(pool_fail::is_link(Some(ErrorCode::Unreachable)) && !pool_fail::is_link(None));
    // Сбой узла — одна запись; удачное чтение пула снимает «записано».
    assert!(f.node_down());
    assert!(!f.node_down());
    f.clear(&r);
    assert!(
        f.node_down(),
        "после удачного чтения новый сбой узла снова пишется"
    );
    // Н7: уведомление — один раз на эпизод.
    f.record(&r, node, 1_000, 60_000, t0);
    assert!(f.tell(&r) && !f.tell(&r));
    f.clear(&r);
    f.record(&r, node, 1_000, 60_000, t0);
    assert!(f.tell(&r), "новый эпизод — новое уведомление");
}

#[test]
fn huge_pause_does_not_panic_and_marks_are_cleaned() {
    let now = Instant::now();
    let t = later(now, u64::MAX);
    assert!(t >= now, "Н3: без паники при огромной паузе");
    assert_eq!(later(now, 1_000), now + Duration::from_millis(1_000));
    let f = Fails::default();
    let a =
        PoolRef::Contract(Address::from_str("0x00000000000000000000000000000000000000aa").unwrap());
    let b =
        PoolRef::Contract(Address::from_str("0x00000000000000000000000000000000000000bb").unwrap());
    let why = (Some(ErrorCode::SwapReverted), "откат");
    let (x, _) = f.record(&a, why, 1_000, u64::MAX, now);
    assert_eq!(x.pause_ms, 1_000);
    for _ in 0..70 {
        f.record(&a, why, 1_000, u64::MAX, now);
    }
    f.record(&b, why, 1_000, u64::MAX, now);
    assert!(f.any());
    f.forget(std::slice::from_ref(&a));
    assert!(
        f.get(&a).is_none() && f.get(&b).is_some(),
        "Н6: удалённый пул — без отметки"
    );
    f.keep_only(&HashSet::new());
    assert!(!f.any(), "Н6: не найденные подпиской — без отметок");
}

#[test]
fn requeue_keeps_setting_abandoned_when_isolated() {
    let mut missing: Vec<Missing> = Vec::new();
    let mut abandoned = vec![
        Abandoned {
            i: 1,
            setting: Some("dex.load_batch".into()),
        },
        Abandoned {
            i: 2,
            setting: None,
        },
    ];
    pool_fail::requeue(true, &mut missing, &mut abandoned, 1);
    assert!(
        missing.is_empty(),
        "ждёт настройку — вернётся, когда её зададут (revive.rs)"
    );
    pool_fail::requeue(true, &mut missing, &mut abandoned, 2);
    pool_fail::requeue(true, &mut missing, &mut abandoned, 2);
    assert_eq!(
        missing.iter().map(|m| m.i).collect::<Vec<_>>(),
        vec![2],
        "в очереди один раз"
    );
    assert_eq!(abandoned.iter().map(|a| a.i).collect::<Vec<_>>(), vec![1]);
}

#[tokio::test]
async fn event_lanes_finish_and_shutdown_without_node() {
    let mut lanes = EventLanes::new();
    let mut m = Members {
        pools: Vec::new(),
        deep: Vec::new(),
        missing: Vec::new(),
        abandoned: vec![Abandoned {
            i: 3,
            setting: None,
        }],
        loaded: true,
    };
    assert!(!lanes.running());
    let (_tx, rx) = mpsc::unbounded_channel::<Msg>();
    assert!(lanes.finish(&mut m, Ok((3, rx, Ok(true)))).is_ok());
    assert_eq!(
        m.missing.iter().map(|x| x.i).collect::<Vec<_>>(),
        vec![3],
        "итог true — в очередь повтора"
    );
    assert!(m.abandoned.is_empty());
    let (_tx, rx) = mpsc::unbounded_channel::<Msg>();
    assert!(lanes.finish(&mut m, Ok((4, rx, Ok(false)))).is_ok());
    assert_eq!(m.missing.len(), 1, "итог false — очередь не меняется");
    let (_tx, rx) = mpsc::unbounded_channel::<Msg>();
    let r = lanes.finish(&mut m, Ok((5, rx, Err(End::Broken("сбой".into())))));
    assert!(
        matches!(r, Err(End::Broken(_))),
        "ошибка события — конец сессии"
    );
    lanes.shutdown().await;
    assert!(!lanes.running());
}

#[test]
fn catalog_has_review_settings_and_event() {
    for name in ["dex.pool_unread_alert_s", "dex.per_block_skip_same"] {
        let def = catalog::find(name).unwrap_or_else(|| panic!("{name} в каталоге"));
        assert!(def.optional, "{name}: необязательная");
        assert!(
            matches!(def.venue_for, VenueFor::Dex),
            "{name}: только площадка DEX"
        );
    }
    assert!(arb::notify::catalog::find("dex.pool_unread").is_some());
    let h = arb::notify::heads::head("dex.pool_unread", "");
    assert!(h.contains("Пул DEX не читается"), "{h}");
    assert!(arb::notify::journal_off::OWN_RECORD.contains(&"dex.pool_unread"));
}
