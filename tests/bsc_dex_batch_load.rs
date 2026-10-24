//! Файл: tests/bsc_dex_batch_load.rs
//! Что делает: пачечное чтение состояния пулов (решение 61, pools/load_batch*.rs) на ЖИВЫХ ответах узла BSC:
//! блок 0x7b936ae, bsc-dataseed.bnbchain.org, 2026-10-25 (tests/data/indexers_2026-10-24/arb_node_responses_3_2026-10-25.json,
//! записано tests/recorders/record_node_calls_3.js; пулы CAKE и ETH PancakeSwap V2/V3, Uniswap V3, PancakeSwap Infinity CL,
//! Uniswap V4). Проверяется:
//! - вызовы ступеней, которые строит бот (головы, слова тиков по тику из ответа, тики CL по битам из ответа), — ровно
//!   записанные вызовы;
//! - пачка Multicall3.aggregate3 — кодировщик бота даёт записанный запрос, а её ответ по вызовам совпадает с
//!   ответами отдельных eth_call;
//! - итог разбора (состояние и карта тиков) по ответам пачки равен итогу по отдельным ответам;
//! - откат одного вызова в пачке задевает только его пул — на записи 2026-10-24 (aggregate3 проверки пулов с
//!   настоящими откатами fee() / tickSpacing() / lmPool() у пар V2 и пулов Uniswap V3).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{pools::{load_batch*, families}, node::{multicall, call}}, arb::settings
//! (настоящая SQLite во временной папке). Настройки теста — значения записи (dex.tick_words = 2, адреса TickLens
//! и StateView — официальные страницы адресов, research/flow/bsc_dex.md и bsc_v4.md).

use std::collections::HashMap;
use std::str::FromStr;

use alloy_primitives::{Address, B256, Bytes};
use serde_json::{Value, json};
use tokio::sync::broadcast;

use arb::core::types::PoolRef;
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::node::multicall;
use arb::venues::bsc_dex::pools::families;
use arb::venues::bsc_dex::pools::load_batch::{Stage, Want};
use arb::venues::bsc_dex::pools::load_batch_v2v3::{self as v23, PoolStateTick};
use arb::venues::bsc_dex::pools::load_v2v3::word_range;
use arb::venues::bsc_dex::pools::{load_batch_cl as cl, load_batch_ticks as t3};

const DIR: &str = "tests/data/indexers_2026-10-24/";

/// Записанные вызовы: имя → (контракт, данные запроса, байты ответа).
type Rec = HashMap<String, (String, String, Bytes)>;

fn record() -> (Rec, Vec<Value>) {
    let text =
        std::fs::read_to_string(format!("{DIR}arb_node_responses_3_2026-10-25.json")).unwrap();
    let doc: Value = serde_json::from_str(&text).unwrap();
    let list = doc["results"].as_array().unwrap().clone();
    let rec = list
        .iter()
        .filter(|r| r["request"]["method"] == "eth_call")
        .map(|r| {
            let p = &r["request"]["params"][0];
            let out = Bytes::from_str(r["response"]["result"].as_str().unwrap()).unwrap();
            let to = p["to"].as_str().unwrap().to_lowercase();
            (
                r["name"].as_str().unwrap().to_string(),
                (to, p["input"].as_str().unwrap().to_lowercase(), out),
            )
        })
        .collect();
    (rec, list)
}

/// Окружение на настоящей SQLite с настройками записи.
async fn ctx() -> Ctx {
    let d = std::env::temp_dir().join(format!("arb-batch-load-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db, changes, journal.clone()).await.unwrap();
    let c = Ctx {
        raw: RawLog::start(d.join("raw"), settings.clone()),
        pace: Default::default(),
        failover: Default::default(),
        index: Default::default(),
        settings,
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        http: reqwest::Client::new(),
        journal,
    };
    let v = Scope::Venue("bsc_dex".into());
    let list = [
        ("dex.tick_words", v, json!(2)),
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
    c
}

/// Пулы записи: (имя, вид dex.kind, ссылка, tickSpacing) — из списка ступени 1 (node_calls_3_2026-10-24.json).
fn pools() -> Vec<(String, &'static families::Family, PoolRef, Option<i32>)> {
    let text = std::fs::read_to_string("tests/data/node_calls_3_2026-10-24.json").unwrap();
    let doc: Value = serde_json::from_str(&text).unwrap();
    let cl_manager: Address = "0xa0ffb9c1ce1fe56963b0321b32e7a0302114058b"
        .parse()
        .unwrap();
    let v4_manager: Address = "0x28e2ea090877bf75740558f6bfb36a5ffee9e9df"
        .parse()
        .unwrap();
    doc["pools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            let tag = p["tag"].as_str().unwrap().to_string();
            let dex = tag.split(' ').nth(1).unwrap().to_string();
            let (kind, manager) = match p["kind"].as_str().unwrap() {
                "v2" => ("pancakeswap_v2", None),
                "pcs_v3" => ("pancakeswap_v3", None),
                "uni_v3" => ("uniswap_v3", None),
                "infinity_cl" => ("pancakeswap_infinity_cl", Some(cl_manager)),
                _ => ("uniswap_v4", Some(v4_manager)),
            };
            let f = families::make(&dex, kind).unwrap();
            let reference = match manager {
                None => PoolRef::Contract(p["addr"].as_str().unwrap().parse().unwrap()),
                Some(manager) => PoolRef::Singleton {
                    manager,
                    id: p["id"].as_str().unwrap().parse::<B256>().unwrap(),
                },
            };
            let sp = p["spacing"].as_i64().unwrap() as i32;
            (tag, f, reference, (sp > 0).then_some(sp))
        })
        .collect()
}

fn want<'a>(p: &'a (String, &'static families::Family, PoolRef, Option<i32>)) -> Want<'a> {
    Want {
        family: p.1,
        reference: &p.2,
        spacing: p.3,
        step: None,
        usdt: Address::ZERO,
        token1: Address::ZERO,
    }
}

/// Записанные ответы ступени `k` по вызовам: только отдельные eth_call, в порядке записи.
fn singles(list: &[Value], k: u8) -> Vec<(String, String, Bytes)> {
    list.iter()
        .filter_map(|r| {
            let n = r["name"].as_str()?;
            (n.starts_with(&format!("s{k} ")) && !n.contains("aggregate3")).then(|| {
                let p = &r["request"]["params"][0];
                let out = Bytes::from_str(r["response"]["result"].as_str().unwrap()).unwrap();
                (
                    p["to"].as_str().unwrap().to_lowercase(),
                    p["input"].as_str().unwrap().to_lowercase(),
                    out,
                )
            })
        })
        .collect()
}

/// Вызовы ступени, как их строит бот, — сверка с записью по порядку; ответы — из записи. Возвращает: ответы по пулам.
fn check_stage(stage: &Stage, rec: &[(String, String, Bytes)]) -> Vec<Vec<Bytes>> {
    let flat: Vec<&(Address, Bytes)> = stage.iter().flatten().collect();
    assert_eq!(flat.len(), rec.len(), "число вызовов ступени");
    for ((to, d), (rto, rd, _)) in flat.iter().zip(rec) {
        assert_eq!(&format!("{to:#x}"), rto);
        assert_eq!(&d.to_string(), rd);
    }
    let mut it = rec.iter().map(|x| x.2.clone());
    stage
        .iter()
        .map(|c| c.iter().map(|_| it.next().unwrap()).collect())
        .collect()
}

/// Пачки aggregate3 ступени `k`: запрос = кодировщик бота, ответы по вызовам = отдельные ответы.
fn check_aggregates(
    list: &[Value],
    k: u8,
    calls: &[(Address, Bytes)],
    single: &[Bytes],
    batch: usize,
) -> usize {
    let parts: Vec<&Value> = list
        .iter()
        .filter(|r| {
            r["name"]
                .as_str()
                .unwrap()
                .starts_with(&format!("s{k} Multicall3 aggregate3"))
        })
        .collect();
    assert_eq!(parts.len(), calls.len().div_ceil(batch));
    let mut n = 0;
    for (i, (part, r)) in calls.chunks(batch).zip(parts).enumerate() {
        assert_eq!(
            r["request"]["params"][0]["input"].as_str().unwrap(),
            multicall::encode(part).to_string()
        );
        let out = Bytes::from_str(r["response"]["result"].as_str().unwrap()).unwrap();
        for (j, got) in multicall::decode(&out, part.len())
            .unwrap()
            .into_iter()
            .enumerate()
        {
            assert_eq!(
                got.unwrap(),
                single[i * batch + j],
                "s{k} пачка {i} вызов {j}"
            );
            n += 1;
        }
    }
    n
}

#[tokio::test]
async fn batch_stages_match_recorded_calls_and_single_answers() {
    let c = ctx().await;
    let (_, list) = record();
    let pools = pools();
    let wants: Vec<Want> = pools.iter().map(want).collect();
    // Ступень 1.
    let s1: Stage = wants.iter().map(|w| v23::head(&c, w).unwrap()).collect();
    let a1 = check_stage(&s1, &singles(&list, 1));
    let heads: Vec<PoolStateTick> = wants
        .iter()
        .zip(&a1)
        .map(|(w, b)| v23::parse_head(w, b).unwrap())
        .collect();
    // Ступень 2: слова по тику из ответа.
    let mut ranges = vec![(0i16, 0i16); wants.len()];
    let s2: Stage = wants
        .iter()
        .zip(&heads)
        .enumerate()
        .map(|(i, (w, h))| match h {
            PoolStateTick::Ticked(_, tick) => {
                ranges[i] = word_range(&c, *tick, w.spacing.unwrap()).unwrap();
                v23::words(&c, w, ranges[i]).unwrap()
            }
            PoolStateTick::V2(_) => Vec::new(),
        })
        .collect();
    let a2 = check_stage(&s2, &singles(&list, 2));
    // Ступень 3: тики CL по битам.
    let mut set = vec![Vec::new(); wants.len()];
    let s3: Stage = wants
        .iter()
        .enumerate()
        .map(|(i, w)| match w.family.kind {
            arb::core::types::PoolKind::Cl => {
                set[i] = cl::set_ticks(w, ranges[i].0, &a2[i], w.spacing.unwrap()).unwrap();
                t3::tick_calls(&c, w, &set[i]).unwrap()
            }
            _ => Vec::new(),
        })
        .collect();
    let a3 = check_stage(&s3, &singles(&list, 3));
    // Карты тиков разбираются без ошибок; у пулов с ликвидностью есть тики.
    let mut with_ticks = 0;
    for (i, w) in wants.iter().enumerate() {
        let map = match w.family.kind {
            arb::core::types::PoolKind::V3 => v23::v3_ticks(&a2[i]).unwrap(),
            arb::core::types::PoolKind::Cl => t3::tick_data(w, &set[i], &a3[i]).unwrap(),
            _ => continue,
        };
        with_ticks += usize::from(!map.is_empty());
    }
    assert!(with_ticks > 0);
    // Пачки aggregate3: 200 вызовов — значение dex.load_batch записи.
    let mut n = 0;
    for (k, stage) in [(1u8, &s1), (2, &s2), (3, &s3)] {
        let calls: Vec<(Address, Bytes)> = stage.iter().flatten().cloned().collect();
        let single: Vec<Bytes> = singles(&list, k).into_iter().map(|x| x.2).collect();
        n += check_aggregates(&list, k, &calls, &single, 200);
    }
    assert_eq!(n, 58 + 115 + 879);
}

#[test]
fn revert_in_batch_hits_only_its_pool() {
    // Запись 2026-10-24: aggregate3 всех вызовов проверки пулов CAKE и ETH (node_calls_2026-10-24.json) — у пар V2 fee(),
    // tickSpacing(), lmPool() и у пулов Uniswap V3 lmPool() откатываются (это ответы контрактов, не подставы).
    let calls: Value = serde_json::from_str(
        &std::fs::read_to_string("tests/data/node_calls_2026-10-24.json").unwrap(),
    )
    .unwrap();
    let resp: Value = serde_json::from_str(
        &std::fs::read_to_string(format!("{DIR}arb_node_responses_2026-10-24.json")).unwrap(),
    )
    .unwrap();
    let agg = resp["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"].as_str().unwrap().contains("aggregate3"))
        .unwrap();
    let names: Vec<String> = calls["calls"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| {
            c["method"] == "eth_call" && !c["name"].as_str().unwrap().contains("aggregate3")
        })
        .map(|c| c["name"].as_str().unwrap().to_string())
        .collect();
    let out = Bytes::from_str(agg["response"]["result"].as_str().unwrap()).unwrap();
    let pieces: Vec<_> = multicall::decode(&out, names.len())
        .unwrap()
        .into_iter()
        .map(Ok)
        .collect();
    // «Пулы» — вызовы одного пула подряд (по имени без последнего слова).
    let mut stage: Stage = Vec::new();
    let mut owners: Vec<String> = Vec::new();
    for n in &names {
        let owner = n.rsplit_once(' ').unwrap().0.to_string();
        if owners.last() != Some(&owner) {
            owners.push(owner);
            stage.push(Vec::new());
        }
        stage
            .last_mut()
            .unwrap()
            .push((Address::ZERO, Bytes::new()));
    }
    let reverted: Vec<bool> = pieces.iter().map(|p| matches!(p, Ok(Err(_)))).collect();
    let answers = v23::split(&stage, pieces);
    let (mut bad, mut good) = (0, 0);
    let mut k = 0;
    for (calls, ans) in stage.iter().zip(&answers) {
        let any = reverted[k..k + calls.len()].iter().any(|r| *r);
        k += calls.len();
        assert_eq!(ans.is_err(), any);
        if any { bad += 1 } else { good += 1 }
    }
    assert!(
        bad > 0 && good > 0,
        "в записи есть и пулы с откатом, и без: {bad} / {good}"
    );
}
