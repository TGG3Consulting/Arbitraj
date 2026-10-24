//! Файл: tests/bsc_dex_node_calls_3.rs
//! Что делает: запись ЖИВЫХ ответов узла BSC для пачечного чтения состояния пулов (решение 61, PR #47,
//! pools/load_batch.rs). Пулы CAKE и ETH официальных DEX из записи GeckoTerminal 2026-10-24 (PancakeSwap V2/V3,
//! Uniswap V3, PancakeSwap Infinity CL через CLPoolManager, Uniswap V4 через StateView); tickSpacing — из уже
//! записанных ответов узла (tickSpacing(), poolIdToPoolKey, poolKeys). Те же три ступени, что у #47:
//!   1) V2 getReserves; V3 slot0 + liquidity; CL getSlot0 + getLiquidity;
//!   2) слова тиков вокруг текущего тика (dex.tick_words): V3 — TickLens.getPopulatedTicksInWord, CL — слова битмапа;
//!   3) CL — данные установленных тиков (getPoolTickInfo / getTickLiquidity).
//!
//! Каждый вызов — отдельным eth_call И теми же пачками Multicall3.aggregate3, что шлёт #47 (вызовы ступени всех пулов
//! подряд, части по dex.verify_batch). Ступени 2 и 3 зависят от ответов предыдущей (текущий тик, установленные биты),
//! поэтому заранее их не перечислить: write_node_calls_3 пишет точный список ступени 1, а record_node_calls_3 проходит
//! все три ступени на узле за один запуск на одном закреплённом блоке.
//! Кто вызывает: человек, вручную (`--ignored`), не CI.
//! От чего зависит: arb::venues::bsc_dex::{node::multicall, pools::{abi_*, key, load_cl, ticks}}.
//! Настройки (переменные окружения — значения настроек бота, не зашиты): ARB_TICK_WORDS = dex.tick_words,
//! ARB_BATCH = dex.verify_batch; для записи — ARB_NODE_URL (адрес узла HTTP). Адреса контрактов — официальные страницы
//! адресов (research/flow/bsc_dex.md: TickLens PancakeSwap V3 и Uniswap V3; bsc_v4.md: StateView, CLPoolManager).
//!
//! Запуск: ARB_TICK_WORDS=2 ARB_BATCH=200 cargo test --test bsc_dex_node_calls_3 write_node_calls_3 -- --ignored
//!         ARB_NODE_URL=https://… ARB_TICK_WORDS=2 ARB_BATCH=200 cargo test --test bsc_dex_node_calls_3 record_node_calls_3 -- --ignored

use std::collections::HashMap;
use std::str::FromStr;

use alloy::sol_types::SolCall;
use alloy_primitives::aliases::I24;
use alloy_primitives::{Address, B256, Bytes, U256, address};
use serde_json::{Value, json};

use arb::venues::bsc_dex::node::multicall;
use arb::venues::bsc_dex::pools::abi_check::{
    IInfinityKeys, IPoolImmutables, IPositionManagerKeys,
};
use arb::venues::bsc_dex::pools::abi_v2v3::{
    IPancakeV3Pool, ITickLens, IUniswapV3Pool, IV2Pair, IV3Pool,
};
use arb::venues::bsc_dex::pools::abi_v4::{IInfinityCl, IStateView};
use arb::venues::bsc_dex::pools::{key, load_cl, ticks::TickMap};

const DIR: &str = "tests/data/indexers_2026-10-24/";
const MULTICALL3: Address = address!("0xcA11bde05977b3631167028862bE2a173976CA11");
const CL_MANAGER: Address = address!("0xa0FfB9c1CE1Fe56963B0321B32E7A0302114058b");
const STATE_VIEW: Address = address!("0xd13dd3d6e93f276fafc9db9e6bb47c1180aee0c4");
const PCS_TICK_LENS: Address = address!("0x9a489505a00cE272eAa5e07Dba6491314CaE3796");
const UNI_TICK_LENS: Address = address!("0xD9270014D396281579760619CCf4c3af0501A47C");

/// Вид пула для записи.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    V2,
    PcsV3,
    UniV3,
    InfinityCl,
    UniV4,
}

/// Пул для записи: имя, вид, адрес (V2/V3) или PoolId (CL), tickSpacing.
struct P {
    tag: String,
    kind: Kind,
    addr: Address,
    id: B256,
    spacing: i32,
}

/// Значение переменной окружения-числа (значение настройки бота). Паника с понятным текстом, если не задана.
fn env_num(name: &str) -> i64 {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("задайте {name} (значение настройки бота)"))
        .parse()
        .unwrap()
}

/// Ответы первой записи (tickSpacing, ключи пулов): имя → байты.
fn first_answers() -> HashMap<String, Bytes> {
    let text = std::fs::read_to_string(format!("{DIR}arb_node_responses_2026-10-24.json")).unwrap();
    let rec: Value = serde_json::from_str(&text).unwrap();
    rec["results"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| {
            let h = r["response"]["result"].as_str()?;
            Some((r["name"].as_str()?.to_string(), Bytes::from_str(h).ok()?))
        })
        .collect()
}

/// Пулы CAKE и ETH официальных DEX из записи индексатора с известным tickSpacing.
fn pools() -> Vec<P> {
    let a = first_answers();
    let text = std::fs::read_to_string(format!("{DIR}arb_indexers_2026-10-24.json")).unwrap();
    let rec: Value = serde_json::from_str(&text).unwrap();
    let mut out = Vec::new();
    for coin in ["CAKE", "ETH"] {
        let body: Value =
            serde_json::from_str(rec[format!("gt_pools_{coin}_p1")]["body"].as_str().unwrap())
                .unwrap();
        for p in body["data"].as_array().unwrap() {
            let dex = p["relationships"]["dex"]["data"]["id"].as_str().unwrap();
            let pool = p["attributes"]["address"].as_str().unwrap().to_lowercase();
            let tag = format!("{coin} {dex} {pool}");
            let spacing_v3 = || {
                let b = a.get(&format!("{tag} tickSpacing()"))?;
                Some(
                    IPoolImmutables::tickSpacingCall::abi_decode_returns(b)
                        .ok()?
                        .as_i32(),
                )
            };
            let (kind, spacing) = match dex {
                "pancakeswap_v2" => (Kind::V2, Some(0)),
                "pancakeswap-v3-bsc" => (Kind::PcsV3, spacing_v3()),
                "uniswap-bsc" => (Kind::UniV3, spacing_v3()),
                "pancakeswap-infinity-clmm" => {
                    let s = a
                        .get(&format!("{tag} CLPoolManager poolIdToPoolKey"))
                        .and_then(|b| {
                            let k =
                                IInfinityKeys::poolIdToPoolKeyCall::abi_decode_returns(b).ok()?;
                            Some(key::infinity_tick_spacing(k.parameters))
                        });
                    (Kind::InfinityCl, s)
                }
                "uniswap-v4-bsc" => {
                    let s = a
                        .get(&format!("{tag} PositionManager poolKeys"))
                        .and_then(|b| {
                            let k =
                                IPositionManagerKeys::poolKeysCall::abi_decode_returns(b).ok()?;
                            Some(k.tickSpacing.as_i32())
                        });
                    (Kind::UniV4, s)
                }
                _ => continue,
            };
            let Some(spacing) = spacing.filter(|s| kind == Kind::V2 || *s > 0) else {
                continue;
            };
            let (addr, id) = if pool.len() == 42 {
                (pool.parse().unwrap(), B256::ZERO)
            } else {
                (Address::ZERO, pool.parse().unwrap())
            };
            out.push(P {
                tag,
                kind,
                addr,
                id,
                spacing,
            });
        }
    }
    out
}

/// Один вызов ступени: имя, контракт, данные.
type C = (String, Address, Bytes);

fn enc(name: String, to: Address, d: Vec<u8>) -> C {
    (name, to, Bytes::from(d))
}

/// Ступень 1 пула (как load_batch_v2v3::head / load_batch_cl::head).
fn head(p: &P) -> Vec<C> {
    let t = &p.tag;
    match p.kind {
        Kind::V2 => vec![enc(
            format!("{t} getReserves"),
            p.addr,
            IV2Pair::getReservesCall {}.abi_encode(),
        )],
        Kind::PcsV3 | Kind::UniV3 => {
            let s = if p.kind == Kind::PcsV3 {
                IPancakeV3Pool::slot0Call {}.abi_encode()
            } else {
                IUniswapV3Pool::slot0Call {}.abi_encode()
            };
            vec![
                enc(format!("{t} slot0"), p.addr, s),
                enc(
                    format!("{t} liquidity"),
                    p.addr,
                    IV3Pool::liquidityCall {}.abi_encode(),
                ),
            ]
        }
        Kind::InfinityCl => vec![
            enc(
                format!("{t} CLPoolManager getSlot0"),
                CL_MANAGER,
                IInfinityCl::getSlot0Call { id: p.id }.abi_encode(),
            ),
            enc(
                format!("{t} CLPoolManager getLiquidity"),
                CL_MANAGER,
                IInfinityCl::getLiquidityCall { id: p.id }.abi_encode(),
            ),
        ],
        Kind::UniV4 => vec![
            enc(
                format!("{t} StateView getSlot0"),
                STATE_VIEW,
                IStateView::getSlot0Call { poolId: p.id }.abi_encode(),
            ),
            enc(
                format!("{t} StateView getLiquidity"),
                STATE_VIEW,
                IStateView::getLiquidityCall { poolId: p.id }.abi_encode(),
            ),
        ],
    }
}

/// Текущий тик из ответа slot0 (V2 — нет).
fn tick(p: &P, b: &[u8]) -> Option<i32> {
    Some(match p.kind {
        Kind::V2 => return None,
        Kind::PcsV3 => IPancakeV3Pool::slot0Call::abi_decode_returns(b)
            .ok()?
            .tick
            .as_i32(),
        Kind::UniV3 => IUniswapV3Pool::slot0Call::abi_decode_returns(b)
            .ok()?
            .tick
            .as_i32(),
        Kind::InfinityCl => IInfinityCl::getSlot0Call::abi_decode_returns(b)
            .ok()?
            .tick
            .as_i32(),
        Kind::UniV4 => IStateView::getSlot0Call::abi_decode_returns(b)
            .ok()?
            .tick
            .as_i32(),
    })
}

/// Слова [w − n, w + n] в пределах int16 (как load_v2v3::word_range).
fn words(tick: i32, spacing: i32, n: i64) -> (i16, i16) {
    let w = i32::from(TickMap::word_of(tick, spacing));
    let n = n.min(i16::MAX as i64) as i32;
    (
        (w - n).max(i16::MIN as i32) as i16,
        (w + n).min(i16::MAX as i32) as i16,
    )
}

/// Ступень 2 пула (как load_batch_v2v3::words / load_batch_cl::word_calls).
fn stage2(p: &P, (lo, hi): (i16, i16)) -> Vec<C> {
    let t = &p.tag;
    (lo..=hi)
        .map(|w| match p.kind {
            Kind::PcsV3 | Kind::UniV3 => {
                let lens = if p.kind == Kind::PcsV3 {
                    PCS_TICK_LENS
                } else {
                    UNI_TICK_LENS
                };
                let d = ITickLens::getPopulatedTicksInWordCall {
                    pool: p.addr,
                    tickBitmapIndex: w,
                };
                enc(format!("{t} TickLens word {w}"), lens, d.abi_encode())
            }
            Kind::InfinityCl => enc(
                format!("{t} CLPoolManager getPoolBitmapInfo {w}"),
                CL_MANAGER,
                IInfinityCl::getPoolBitmapInfoCall { id: p.id, word: w }.abi_encode(),
            ),
            _ => enc(
                format!("{t} StateView getTickBitmap {w}"),
                STATE_VIEW,
                IStateView::getTickBitmapCall {
                    poolId: p.id,
                    tick: w,
                }
                .abi_encode(),
            ),
        })
        .collect()
}

/// Ступень 3 пула CL по установленным тикам.
fn stage3(p: &P, ticks: &[i32]) -> Vec<C> {
    let t = &p.tag;
    ticks
        .iter()
        .map(|&k| {
            let tick = I24::try_from(k).unwrap();
            if p.kind == Kind::InfinityCl {
                let d = IInfinityCl::getPoolTickInfoCall { id: p.id, tick };
                enc(
                    format!("{t} CLPoolManager getPoolTickInfo {k}"),
                    CL_MANAGER,
                    d.abi_encode(),
                )
            } else {
                let d = IStateView::getTickLiquidityCall { poolId: p.id, tick };
                enc(
                    format!("{t} StateView getTickLiquidity {k}"),
                    STATE_VIEW,
                    d.abi_encode(),
                )
            }
        })
        .collect()
}

/// Запросы ступени: каждый вызов отдельно, затем пачки aggregate3 по `batch` (вызовы всех пулов подряд, как #47).
fn requests(stage: u8, calls: &[C], batch: usize, block: &str) -> Vec<Value> {
    let one = |name: String, to: &Address, d: &Bytes| json!({ "name": name, "method": "eth_call", "params": [{ "to": to, "input": d }, block] });
    let mut out: Vec<Value> = calls
        .iter()
        .map(|(n, to, d)| one(format!("s{stage} {n}"), to, d))
        .collect();
    for (i, part) in calls.chunks(batch).enumerate() {
        let list: Vec<(Address, Bytes)> = part.iter().map(|(_, a, d)| (*a, d.clone())).collect();
        let name = format!(
            "s{stage} Multicall3 aggregate3 часть {i} ({} вызовов)",
            part.len()
        );
        out.push(one(name, &MULTICALL3, &multicall::encode(&list)));
    }
    out
}

#[test]
#[ignore = "вручную: пишет точный список вызовов ступени 1"]
fn write_node_calls_3() {
    let batch = env_num("ARB_BATCH") as usize;
    let calls: Vec<C> = pools().iter().flat_map(head).collect();
    let mut out = vec![json!({ "name": "block", "method": "eth_blockNumber", "params": [] })];
    out.extend(requests(1, &calls, batch, "latest"));
    let doc = json!({
        "about": format!("Ступень 1 пачечного чтения (#47): каждый вызов отдельно и пачками Multicall3.aggregate3 по dex.verify_batch = {batch}. Ступени 2–3 зависят от ответов (тик, биты) — их пишет record_node_calls_3 за один проход на закреплённом блоке. \"latest\" заменить номером блока из первого ответа."),
        "calls": out,
    });
    std::fs::write(
        "tests/data/node_calls_3_2026-10-24.json",
        serde_json::to_string_pretty(&doc).unwrap(),
    )
    .unwrap();
}

/// JSON-RPC запрос к узлу. Возвращает: ответ целиком (result или error).
async fn rpc(http: &reqwest::Client, url: &str, id: usize, req: &Value) -> Value {
    let body =
        json!({ "jsonrpc": "2.0", "id": id, "method": req["method"], "params": req["params"] });
    http.post(url)
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "вручную: пишет живые ответы узла для пачечного чтения (нужен ARB_NODE_URL)"]
async fn record_node_calls_3() {
    let url = std::env::var("ARB_NODE_URL").expect("задайте ARB_NODE_URL");
    let (n, batch) = (env_num("ARB_TICK_WORDS"), env_num("ARB_BATCH") as usize);
    let http = reqwest::Client::new();
    let mut results = Vec::new();
    let head_req = json!({ "name": "block", "method": "eth_blockNumber", "params": [] });
    let resp = rpc(&http, &url, 0, &head_req).await;
    let block = resp["result"].as_str().unwrap().to_string();
    results.push(json!({ "name": "block", "request": head_req, "response": resp }));
    let pools = pools();
    // Ответы по имени вызова (отдельные eth_call) — из них строится следующая ступень.
    let mut got: HashMap<String, Bytes> = HashMap::new();
    let run = |stage: u8, calls: Vec<C>| requests(stage, &calls, batch, &block);
    let record =
        async |reqs: Vec<Value>, results: &mut Vec<Value>, got: &mut HashMap<String, Bytes>| {
            for (i, r) in reqs.into_iter().enumerate() {
                let resp = rpc(&http, &url, i + 1, &r).await;
                if let Some(h) = resp["result"].as_str() {
                    got.insert(
                        r["name"].as_str().unwrap().to_string(),
                        Bytes::from_str(h).unwrap(),
                    );
                }
                results.push(json!({ "name": r["name"], "request": r, "response": resp }));
            }
        };
    let reqs = run(1, pools.iter().flat_map(head).collect());
    record(reqs, &mut results, &mut got).await;
    let mut ranges = HashMap::new();
    let mut s2 = Vec::new();
    for p in &pools {
        let first = head(p).remove(0).0;
        if let Some(t) = got.get(&format!("s1 {first}")).and_then(|b| tick(p, b)) {
            let r = words(t, p.spacing, n);
            ranges.insert(p.tag.clone(), r);
            s2.extend(stage2(p, r));
        }
    }
    let reqs = run(2, s2);
    record(reqs, &mut results, &mut got).await;
    let mut s3 = Vec::new();
    for p in pools
        .iter()
        .filter(|p| matches!(p.kind, Kind::InfinityCl | Kind::UniV4))
    {
        let Some(&r) = ranges.get(&p.tag) else {
            continue;
        };
        let ws: Option<Vec<U256>> = stage2(p, r)
            .iter()
            .map(|(name, _, _)| {
                let b = got.get(&format!("s2 {name}"))?;
                if p.kind == Kind::InfinityCl {
                    IInfinityCl::getPoolBitmapInfoCall::abi_decode_returns(b).ok()
                } else {
                    IStateView::getTickBitmapCall::abi_decode_returns(b).ok()
                }
            })
            .collect();
        if let Some(ws) = ws {
            s3.extend(stage3(p, &load_cl::set_ticks(r.0, &ws, p.spacing)));
        }
    }
    let reqs = run(3, s3);
    record(reqs, &mut results, &mut got).await;
    let doc = json!({
        "about": format!("Живые ответы узла для пачечного чтения состояния пулов (#47), блок {block}, dex.tick_words = {n}, dex.verify_batch = {batch}. Записано tests/bsc_dex_node_calls_3.rs (record_node_calls_3). s1/s2/s3 — ступени; каждый вызов отдельно и теми же пачками Multicall3.aggregate3."),
        "block": block,
        "results": results,
    });
    let day = chrono::Utc::now().format("%Y-%m-%d");
    std::fs::write(
        format!("{DIR}arb_node_responses_3_{day}.json"),
        serde_json::to_string_pretty(&doc).unwrap(),
    )
    .unwrap();
}
