//! Файл: tests/arbitrum_wombat_live.rs
//! Что делает: решение 484 — пулы Wombat Exchange на записи своего узла Arbitrum (tools/arbitrum_record/wombat.py):
//! (1) состояние собирается тем же кодом, что у бота (load_wombat_calls: head_calls → parse_head, asset_calls →
//! parse_assets, detail_calls → parse_detail, build), ответы — по (адрес, calldata); реализация прокси и паузы активов —
//! по тем же слотам (abi_wombat: implementation_slot, paused_slot); пул с реализацией не из сверенных списков
//! (Cross Chain — CrossChainPool на CoreV3) пропускается с выводом причины; (2) слот 201 _pausedAssets = isPaused(монета)
//! у реализаций с геттером; (3) наш выход quotePotentialSwap (math/wombat.rs potential) = ответ пула побайтно, откат
//! пула — наш None; (4) симуляция swap (eth_call с подменой баланса и разрешения): пул отдал выход — наш выход swap
//! (swap_out) тот же; пул откатил — наш None.
//! Кто вызывает: cargo test. До записи — #[ignore].
//! От чего зависит: tests/data/arbitrum_wombat_<дата>/ (pools.json), arb::venues::bsc_dex::{pools::{abi_wombat,
//! load_wombat_calls}, math::wombat}.
//! Реализации (Sourcify 42161 proxyResolution пулов, exact_match; docs/sources/wombat_arbitrum/README.md) — списки ниже —
//! совет docs/FIRST_RUN.md для dex.wombat_implementations / dex.wombat_dynamic_implementations.

use std::path::PathBuf;

use alloy_primitives::{Address, B256, Bytes, U256, address, hex, keccak256};
use serde_json::Value;

use arb::core::types::PoolState;
use arb::venues::bsc_dex::math::wombat::{potential, swap_out};
use arb::venues::bsc_dex::pools::abi_wombat::{implementation_slot, paused_slot};
use arb::venues::bsc_dex::pools::load_wombat_calls as c;

/// Реализации HighCovRatioFeePoolV2 пулов docs.wombat.exchange (Sourcify).
const PLAIN: [Address; 9] = [
    address!("0x82E62f4e174E3C5e1641Df670c91Ac6Ab8541518"),
    address!("0x6A4b5988692F8C47eA3BcCFB8A127288db61f5E1"),
    address!("0x30E27c4E1C9D8845a961CA703bf4B5e46e1F554c"),
    address!("0x3ccC0dC754E380cDA16EF94292F278bbc4Ff463E"),
    address!("0xC096FF2606152eD2A06dd12F15A3c0466Aa5A9fa"),
    address!("0xbEd0509801634CcC29Fa9f2faD764Bf194c7927d"),
    address!("0x9cbeE53A48390A3205B14F6B8e48Ca81Fe105dEE"),
    address!("0x38D2259fDeEdc94Eaf915a872139c985Ce36d5Ad"),
    address!("0x44A0e2AC2c99D5cB2910ec1784DD158b7fa3A642"),
];
/// Реализации DynamicPoolV2 (Sourcify).
const DYNAMIC: [Address; 3] = [
    address!("0x4d1CC84a33462c35ad3dd68756d330CBbA087053"),
    address!("0x3DbF3e359a570D46CF5a663AC92DC7Cae77110FB"),
    address!("0x7585bae98800C621b427Fe045EA3b4052559C11b"),
];

/// Последняя по имени папка записи tests/data/arbitrum_wombat_*. Паника — записи нет.
fn data_dir() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
        .expect("tests/data")
        .filter_map(|d| d.ok().map(|d| d.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("arbitrum_wombat_"))
        })
        .collect();
    dirs.sort();
    dirs.pop().expect("нет записи: tools/arbitrum_record/wombat.py")
}

/// Ответ {"result": "0x…"} → байты; откат или нет ответа — Err(текст).
fn answer(v: Option<&Value>) -> Result<Bytes, String> {
    match v.and_then(|x| x.get("result")).and_then(|h| h.as_str()) {
        Some(h) => Ok(Bytes::from(hex::decode(h).expect("hex"))),
        None => Err(v.map_or_else(|| "нет ответа".to_string(), |x| x["error"].to_string())),
    }
}

/// Ответы записи на вызовы бота по порядку (ключ «адрес|calldata», wombat_calls.py key).
fn lookup(p: &Value, calls: &[(Address, Bytes)]) -> Vec<Result<Bytes, String>> {
    calls
        .iter()
        .map(|(to, d)| answer(p["answers"].get(format!("{to:#x}|0x{}", hex::encode(d)))))
        .collect()
}

/// Слово хранилища записи (ключ «пул|слот»).
fn slot(p: &Value, pool: Address, s: B256) -> B256 {
    let h = p["storage"][format!("{pool:#x}|{s:#x}")]["result"]
        .as_str()
        .expect("слот записан");
    h.parse().expect("слово")
}

/// Первое слово ответа как U256.
fn first(h: &str) -> U256 {
    let b = hex::decode(h).expect("hex");
    U256::from_be_slice(&b[..32])
}

/// Сумма записи (строка десятичная).
fn num(v: &Value) -> U256 {
    U256::from_str_radix(v.as_str().expect("сумма"), 10).expect("сумма")
}

#[test]
#[ignore = "нет записи: tools/arbitrum_record/wombat.py"]
fn wombat_state_quote_and_swap_match_node() {
    let text = std::fs::read_to_string(data_dir().join("pools.json")).expect("pools.json");
    let pools: Value = serde_json::from_str(&text).expect("json");
    let (mut quotes, mut swaps, mut skipped) = (0, 0, Vec::new());
    for p in pools.as_array().expect("список") {
        let pool: Address = p["pool"].as_str().unwrap().parse().unwrap();
        let head = c::parse_head(&lookup(p, &c::head_calls(pool))).expect("ступень 1");
        let imp = Address::from_word(slot(p, pool, implementation_slot()));
        let dynamic = if DYNAMIC.contains(&imp) {
            true
        } else if PLAIN.contains(&imp) {
            false
        } else {
            skipped.push(format!("{} {pool:#x}: реализация {imp:#x} не сверена", p["name"]));
            continue;
        };
        let tokens = head.tokens.clone();
        let assets = c::parse_assets(&lookup(p, &c::asset_calls(pool, &tokens)), &tokens).unwrap();
        let got = lookup(p, &c::detail_calls(&assets, dynamic));
        let sides = c::parse_detail(&got, pool, &tokens, dynamic).expect("ступень 3");
        let paused: Vec<bool> =
            tokens.iter().map(|t| !slot(p, pool, paused_slot(*t)).is_zero()).collect();
        // Слот 201 = isPaused(монета) у реализаций с геттером (откат — геттера нет, сверять нечем).
        for (k, t) in tokens.iter().enumerate() {
            let mut data = keccak256(b"isPaused(address)")[..4].to_vec();
            data.extend_from_slice(&[0u8; 12]);
            data.extend_from_slice(t.as_slice());
            let key = format!("{pool:#x}|0x{}", hex::encode(&data));
            if let Ok(b) = answer(p["answers"].get(key)) {
                let on = !U256::from_be_slice(&b[..32]).is_zero();
                assert_eq!(on, paused[k], "{pool:#x} isPaused({t})");
            }
        }
        let state = |i: usize, j: usize| {
            let st = c::build(&head, &sides, &paused, (tokens[i], tokens[j])).expect("сборка");
            let PoolState::StableSwap(d) = st else {
                panic!("не StableSwap")
            };
            *d
        };
        let idx = |r: &Value, f: &str| r[f].as_u64().unwrap() as usize;
        for q in p["quote"].as_array().into_iter().flatten() {
            let (i, j, dx) = (idx(q, "i"), idx(q, "j"), num(&q["dx"]));
            let ours = potential(&state(i, j), i, j, dx);
            match q.get("result").and_then(|h| h.as_str()) {
                Some(h) => assert_eq!(ours, Some(first(h)), "{pool:#x} quote {i}→{j} {dx}"),
                None => assert_eq!(ours, None, "{pool:#x} quote {i}→{j} {dx}: пул откатил"),
            }
            quotes += 1;
        }
        for s in p["swap_sim"].as_array().into_iter().flatten() {
            if s["override_ok"].as_bool() != Some(true) {
                continue;
            }
            let (i, j, dx) = (idx(s, "i"), idx(s, "j"), num(&s["dx"]));
            let ours = swap_out(&state(i, j), i, j, dx);
            match s["ok"].as_bool() {
                Some(true) => {
                    let h = s["result"].as_str().unwrap();
                    assert_eq!(ours, Some(first(h)), "{pool:#x} swap {i}→{j} {dx}");
                }
                _ => {
                    let why = &s["revert"];
                    assert_eq!(ours, None, "{pool:#x} swap {i}→{j} {dx}: пул откатил {why}");
                }
            }
            swaps += 1;
        }
    }
    println!("пропущены: {skipped:?}; сверено квотером {quotes}, симуляцией {swaps}");
    assert!(quotes > 0 && swaps > 0, "в записи нет сверок");
}
