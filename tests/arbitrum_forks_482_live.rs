//! Файл: tests/arbitrum_forks_482_live.rs
//! Что делает: решение 482 — DEX Arbitrum, подключённые настройками как форки поддержанных видов, на записи своего узла:
//! (1) V2 (Joe DEX / LFJ V1, Rubicon Aquila, Swapsicle V1, Arbitrum Exchange V2): пара принадлежит фабрике (factory(),
//! getPair), у Arbidex swapFee() = 25; наш выход math/v2 с комиссией dex.v2_fee_pct = getAmountsOut роутера (Joe,
//! Aquila) и = выходу, который пара принимает в симуляции обмена (eth_call swap от пустого адреса, вход ровно dx), а
//! выход + 1 пара откатывает (проверка K) — комиссия бота ровно комиссия пары;
//! (2) V3 (Rubicon CLMM, Arbitrum Exchange V3): пул принадлежит фабрике (factory(), getPool), тики TickLens (своего DEX и
//! Uniswap; правило T482-01: свой не совпал — берётся совпавший Uniswap, факт печатается) = ticks(t) самого пула, наш
//! расчёт math/swap_cl = виртуальному квотеру (и QuoterV2 Rubicon) до wei;
//! (3) ZyberSwap Stableswap (saddle_stableswap): разбор бота load_saddle и math/stableswap_v1 = calculateSwap пула.
//! Кто вызывает: cargo test. До записи — #[ignore] (tools/arbitrum_record/forks_482.py, шаги forks_482 и zyber_stable
//! record_steps.py).
//! От чего зависит: tests/data/arbitrum_forks_482_<дата>/, tests/data/arbitrum_zyber_stable_<дата>/,
//! arb::venues::bsc_dex::{math::{v2, swap_cl, stableswap_v1}, pools::{load_batch_v2v3, ticks, load_saddle}}.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::str::FromStr;

use alloy_primitives::{Address, Bytes, U256, hex};
use rust_decimal::Decimal;
use serde_json::Value;

use arb::core::types::PoolState;
use arb::gateway::errors::{ErrorCode, GatewayError};
use arb::venues::bsc_dex::math::stableswap_v1::{Template, exchange_out_t};
use arb::venues::bsc_dex::math::swap_cl::{self, ClState, Version};
use arb::venues::bsc_dex::math::v2;
use arb::venues::bsc_dex::pools::load_batch_v2v3::v3_ticks;
use arb::venues::bsc_dex::pools::load_saddle;
use arb::venues::bsc_dex::pools::ticks::{TickData, TickMap};

/// Последняя по имени папка записи tests/data/<prefix>*. Паника — записи нет.
fn data_dir(prefix: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
        .expect("tests/data")
        .filter_map(|d| d.ok().map(|d| d.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(prefix))
        })
        .collect();
    dirs.sort();
    dirs.pop()
        .expect("нет записи: tools/arbitrum_record/forks_482.py / saddle.py")
}

fn json(prefix: &str, name: &str) -> Value {
    let text = std::fs::read_to_string(data_dir(prefix).join(name)).expect(name);
    serde_json::from_str(&text).expect("json")
}

/// Ответ узла {"result": "0x…"} → hex без 0x; откат — None.
fn res(v: &Value) -> Option<&str> {
    v.get("result")
        .and_then(|h| h.as_str())
        .map(|h| h.trim_start_matches("0x"))
}

/// Слово `i` ответа (беззнаковое).
fn word(raw: &str, i: usize) -> U256 {
    U256::from_str_radix(&raw[64 * i..64 * (i + 1)], 16).unwrap()
}

/// Слово `i` как знаковое (int24 / int128 — дополнение до двух).
fn sword(raw: &str, i: usize) -> i128 {
    let w = word(raw, i);
    if w.bit(255) {
        -((!w).wrapping_add(U256::from(1u8)).to::<u128>() as i128)
    } else {
        w.to::<u128>() as i128
    }
}

fn addr_of(raw: &str) -> Address {
    Address::from_slice(&hex::decode(&raw[24..64]).unwrap())
}

fn addr(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}

fn num(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}

/// Ответ TickLens записи → байты (откат слова — паника: запись неполная).
fn lens_bytes(a: &Value) -> Bytes {
    Bytes::from(hex::decode(res(a).expect("TickLens")).unwrap())
}

/// Сверка тиков TickLens с ticks(t) самого пула: каждый записанный тик с liquidityGross > 0 есть в карте TickLens с
/// теми же liquidityGross / liquidityNet, и в карте нет тиков, которых пул не знает. Возвращает: Ok или первое
/// расхождение текстом.
fn lens_vs_pool(map: &BTreeMap<i32, TickData>, ticks: &[Value]) -> Result<(), String> {
    for t in ticks {
        let raw = res(t).ok_or("ticks(t) без ответа")?;
        let tick = t["tick"].as_i64().unwrap() as i32;
        let (gross, net) = (word(raw, 0).to::<u128>(), sword(raw, 1));
        match map.get(&tick) {
            Some(td) if td.gross == gross && td.net == net => {}
            None if gross == 0 => {}
            got => {
                return Err(format!(
                    "тик {tick}: TickLens {got:?}, пул ({gross}, {net})"
                ));
            }
        }
    }
    Ok(())
}

/// Метка ответа — котировка вида `kind` («virtual» / «quoterV2»).
fn is_quote(a: &Value, kind: &str) -> bool {
    a["mark"][0] == "q" && a["mark"][3] == kind
}

/// Настройки DEX записи (meta.json dexes_v2 / dexes_v3) по id.
fn dex<'a>(meta: &'a Value, list: &str, id: &Value) -> &'a Value {
    meta[list]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| &d["id"] == id)
        .expect("DEX в meta")
}

#[test]
#[ignore = "нет записи: tools/arbitrum_record/forks_482.py"]
fn v2_forks_math_equals_router_and_pair() {
    let meta = json("arbitrum_forks_482_", "meta.json");
    let (mut router, mut sims) = (0, 0);
    for p in json("arbitrum_forks_482_", "v2.json").as_array().unwrap() {
        let d = dex(&meta, "dexes_v2", &p["dex"]);
        let st = &p["state"];
        let case = format!("{} {}", p["dex"], p["pair"]);
        let factory = addr_of(res(&st["factory()"]).unwrap());
        assert_eq!(factory, addr(&d["factory"]), "{case}");
        let registry = addr_of(res(&st["getPair"]).unwrap());
        assert_eq!(registry, addr(&p["pair"]), "{case}: getPair");
        if let Some(m) = d["fee_method"].as_str() {
            let fee = word(res(&st[m]).unwrap(), 0);
            assert_eq!(fee, U256::from(25u8), "{case}: swapFee()");
        }
        let pct = Decimal::from_str(d["fee_pct"].as_str().unwrap()).unwrap();
        let fee = v2::fee_ratio(pct).unwrap();
        let r = res(&st["getReserves()"]).unwrap();
        let (r0, r1) = (word(r, 0), word(r, 1));
        for a in p["answers"].as_array().unwrap() {
            let m = a["mark"].as_array().unwrap();
            let zfo = m[1].as_bool().unwrap();
            let (dx, want) = (num(&m[2]), num(&m[3]));
            let (r_in, r_out) = if zfo { (r0, r1) } else { (r1, r0) };
            let ours = v2::amount_out(dx, r_in, r_out, fee).unwrap();
            match m[0].as_str().unwrap() {
                "router" => {
                    let raw = res(a).expect("getAmountsOut");
                    let got = word(raw, 3);
                    assert_eq!(got, ours, "{case} {zfo} dx {dx}: getAmountsOut");
                    router += 1;
                }
                _ if want == ours => {
                    let taken = res(a).is_some();
                    assert!(taken, "{case} {zfo} dx {dx}: пара откатила наш выход {a}");
                    sims += 1;
                }
                _ => {
                    assert_eq!(want, ours + U256::from(1u8), "{case}: метка симуляции");
                    let taken = res(a).is_some();
                    assert!(!taken, "{case} {zfo} dx {dx}: пара приняла выход + 1");
                }
            }
        }
    }
    assert!(
        router > 0 && sims > 0,
        "в записи нет ответов роутера или симуляций"
    );
}

#[test]
#[ignore = "нет записи: tools/arbitrum_record/forks_482.py"]
fn v3_forks_ticks_and_math_equal_quoters() {
    let meta = json("arbitrum_forks_482_", "meta.json");
    let mut full = 0;
    for p in json("arbitrum_forks_482_", "v3.json").as_array().unwrap() {
        let d = dex(&meta, "dexes_v3", &p["dex"]);
        let st = &p["state"];
        let case = format!("{} {}", p["dex"], p["pool"]);
        let factory = addr_of(res(&st["factory()"]).unwrap());
        assert_eq!(factory, addr(&d["factory"]), "{case}");
        let registry = addr_of(res(&st["getPool"]).unwrap());
        assert_eq!(registry, addr(&p["pool"]), "{case}: getPool");
        let ans = p["answers"].as_array().unwrap();
        // Правило T482-01: TickLens своего DEX (если есть) — основной; не совпал с ticks(t) пула — берётся TickLens
        // Uniswap, если совпал он (факт печатается). Не совпал ни один — ошибка.
        let words = |name: &str| -> Vec<(i64, Bytes)> {
            ans.iter()
                .filter(|a| a["mark"][0] == "lens" && a["mark"][1] == name)
                .map(|a| (a["mark"][2].as_i64().unwrap(), lens_bytes(a)))
                .collect()
        };
        let order: &[&str] = if d["tick_lens"].is_null() {
            &["uniswap"]
        } else {
            &["dex", "uniswap"]
        };
        let mut chosen = None;
        for name in order {
            let main = words(name);
            let raw: Vec<Bytes> = main.iter().map(|(_, b)| b.clone()).collect();
            let map = v3_ticks(&raw).expect("тики TickLens");
            match lens_vs_pool(&map, p["ticks"].as_array().unwrap()) {
                Ok(()) => {
                    println!("{case}: TickLens «{name}» = ticks(t) пула");
                    chosen = Some((main, map));
                    break;
                }
                Err(why) => println!("{case}: TickLens «{name}» ≠ ticks(t) пула — {why}"),
            }
        }
        let (main, map) = chosen.expect("ни один TickLens не совпал с ticks(t) пула");
        let s0 = res(&st["slot0()"]).unwrap();
        let tm = TickMap {
            spacing: sword(res(&st["tickSpacing()"]).unwrap(), 0) as i32,
            ticks: map.into(),
            words: (
                main.first().unwrap().0 as i16,
                main.last().unwrap().0 as i16,
            ),
        };
        let s = ClState {
            sqrt_price: word(s0, 0),
            tick: sword(s0, 1) as i32,
            liquidity: word(res(&st["liquidity()"]).unwrap(), 0).to::<u128>(),
            fee: word(res(&st["fee()"]).unwrap(), 0).to::<u32>(),
            ticks: &tm,
            version: Version::V3,
        };
        for v in ans.iter().filter(|a| is_quote(a, "virtual")) {
            let (zfo, amount) = (v["mark"][1].as_bool().unwrap(), num(&v["mark"][2]));
            let raw = res(v).expect("виртуальный квотер");
            let (used, out) = (word(raw, 0), word(raw, 1));
            let same = |a: &&Value| {
                is_quote(a, "quoterV2")
                    && a["mark"][1] == v["mark"][1]
                    && a["mark"][2] == v["mark"][2]
            };
            let qv2 = ans.iter().find(same).and_then(res);
            if used == amount
                && let Some(qr) = qv2
            {
                let q = word(qr, 0);
                assert_eq!(
                    q, out,
                    "{case} {zfo} {amount}: QuoterV2 ≠ виртуальный квотер"
                );
            }
            let r = swap_cl::swap(&s, zfo, true, amount).expect("расчёт");
            let case = format!("{case} {zfo} {amount}");
            if !r.complete || used != amount {
                assert!(
                    r.amount_out <= out,
                    "{case}: частичный выход больше квотера"
                );
                continue;
            }
            assert_eq!(r.amount_out, out, "{case}: математика ≠ виртуальный квотер");
            full += 1;
        }
    }
    assert!(full > 0, "ни одного полного расчёта V3 в записи");
}

fn bad(what: String) -> GatewayError {
    GatewayError::new(ErrorCode::BadResponse, None, what)
}

/// Ответ saddle.py {"ok": "0x…"} → байты; откат — Err(текст).
fn ok(v: &Value) -> Result<Bytes, String> {
    match v.get("ok").and_then(|h| h.as_str()) {
        Some(h) => Ok(Bytes::from(hex::decode(h).expect("hex"))),
        None => Err(v["error"].as_str().unwrap_or("нет ответа").to_string()),
    }
}

#[test]
#[ignore = "нет записи: tools/arbitrum_record/saddle.py (шаг zyber_stable)"]
fn zyber_stable_math_equals_calculate_swap() {
    let mut checked = 0;
    for p in json("arbitrum_zyber_stable_", "pools.json")
        .as_array()
        .unwrap()
    {
        assert!(p.get("error").is_none(), "пул не записан: {p}");
        let storage = ok(&p["swapStorage"]).unwrap();
        assert_eq!(storage.len(), 7 * 32, "swapStorage — 7 слов (Saddle)");
        let tokens = p["getToken"].as_array().unwrap();
        let mut head: Vec<Result<Bytes, String>> = Vec::new();
        for (k, t) in tokens.iter().enumerate() {
            head.push(ok(t));
            head.push(ok(&p["getTokenBalance"][k]));
        }
        head.push(Err("Out of range".into()));
        let tail = vec![
            ok(&p["getAPrecise"]),
            ok(&p["swapStorage"]),
            ok(&p["paused"]),
        ];
        let coin = |k: usize| Address::from_slice(&ok(&tokens[k]).unwrap()[12..]);
        let (a, b) = (coin(0), coin(1));
        let pair = if a < b { (a, b) } else { (b, a) };
        let parsed = load_saddle::parse(&head, &tail, pair, &bad).unwrap();
        let dec: Vec<u8> = p["decimals"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| U256::from_be_slice(&ok(d).unwrap()).to::<u8>())
            .collect();
        let PoolState::StableSwap(st) = load_saddle::build(parsed, &dec, &bad).unwrap() else {
            panic!("не StableSwap");
        };
        let t = Template {
            amp_precision: st.amp_precision,
            max_rounds: st.max_rounds,
            strict: st.strict,
            pair_d: st.pair_d,
        };
        for q in p["quotes"].as_array().unwrap() {
            let Ok(raw) = ok(q) else { continue };
            let (i, j) = (
                q["i"].as_u64().unwrap() as usize,
                q["j"].as_u64().unwrap() as usize,
            );
            let dx: U256 = q["dx"].as_str().unwrap().parse().unwrap();
            let ours = exchange_out_t(i, j, dx, &st.balances, &st.rates, st.amp, st.fee, &t);
            assert_eq!(ours, Some(U256::from_be_slice(&raw)), "{i}→{j} dx {dx}");
            checked += 1;
        }
    }
    assert!(checked > 0, "ни одного ответа calculateSwap в записи");
}
