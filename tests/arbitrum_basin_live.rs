//! Файл: tests/arbitrum_basin_live.rs
//! Что делает: решение 485 — скважины Basin (вид basin_well) на записи своего узла Arbitrum
//! (tests/data/arbitrum_basin_<дата>/, tools/arbitrum_record/basin.py; #[ignore], пока записи нет):
//! 1. состояние собирается тем же кодом, что у бота (слот ERC-1967 записи, load_basin::calls → ответы по (адрес,
//!    calldata) → decode → state): реализация допущена (клон Aquifer'а или прокси по слоту), функция из списков;
//! 2. проверка скважины (verify_basin_judge first / second на ответах тех же вызовов) — скважина берётся:
//!    calcReserve функции на сдвинутых резервах = наш расчёт (версия функции, A Stable2) или «не сверено» с причиной;
//! 3. наш выход (math/basin_well.rs swap_out) = getSwapOut скважины побайтно на каждой записанной сумме;
//! 4. симуляция swapFrom на тех же суммах (eth_call с подменой баланса и разрешения): скважина отдала выход — наш
//!    выход тот же; откатила — наша котировка None.
//!
//! Кто вызывает: cargo test. До записи — #[ignore].
//! От чего зависит: arb::venues::bsc_dex::{pools::{load_basin, basin_conf, verify_basin_judge}, math::basin_well}.

use std::path::PathBuf;
use std::str::FromStr;

use alloy_primitives::{Address, B256, Bytes, U256, hex};
use serde_json::Value;

use arb::core::types::PoolState;
use arb::venues::bsc_dex::math::basin_well;
use arb::venues::bsc_dex::pools::basin_conf::BasinConf;
use arb::venues::bsc_dex::pools::{load_basin, verify_basin_judge as judge};

/// Последняя по имени папка записи tests/data/arbitrum_basin_*. Паника — записи нет.
fn data_dir() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
        .expect("tests/data")
        .filter_map(|d| d.ok().map(|d| d.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("arbitrum_basin_"))
        })
        .collect();
    dirs.sort();
    dirs.pop()
        .expect("нет записи: tools/arbitrum_record/basin.py")
}

fn read(name: &str) -> Value {
    let text = std::fs::read_to_string(data_dir().join(name)).expect(name);
    serde_json::from_str(&text).expect("json")
}

fn addr(v: &Value) -> Address {
    v.as_str().expect("адрес").parse().expect("адрес")
}

fn addrs(v: &Value) -> Vec<Address> {
    v.as_array().expect("список").iter().map(addr).collect()
}

/// Ключ ответа записи: «адрес|calldata» в нижнем регистре (curve_plain_pools.py key).
fn key(to: Address, data: &[u8]) -> String {
    format!("{to:#x}|0x{}", hex::encode(data))
}

/// Ответ {"result": "0x…"} → байты; откат или нет ответа — Err(текст).
fn answer(v: Option<&Value>) -> Result<Bytes, String> {
    match v.and_then(|x| x.get("result")).and_then(|h| h.as_str()) {
        Some(h) => Ok(Bytes::from(hex::decode(h).expect("hex"))),
        None => Err(v.map_or_else(|| "нет ответа".to_string(), |x| x["error"].to_string())),
    }
}

/// Ответы записи на вызовы по порядку.
fn answers(w: &Value, calls: &[(Address, Bytes)]) -> Vec<Result<Bytes, String>> {
    calls
        .iter()
        .map(|(to, d)| answer(w["answers"].get(key(*to, d))))
        .collect()
}

fn conf(meta: &Value) -> BasinConf {
    BasinConf {
        aquifer: addr(&meta["aquifer"]),
        impls: addrs(&meta["impls"]),
        cp2: addrs(&meta["cp2"]),
        stable2: addrs(&meta["stable2"]),
        luts: addrs(&meta["luts"]),
    }
}

fn u(v: &Value) -> U256 {
    U256::from_str_radix(v.as_str().expect("число"), 10).expect("число")
}

/// Слово ответа eth_call → U256.
fn word(h: &str) -> U256 {
    U256::from_str_radix(h.trim_start_matches("0x"), 16).expect("hex")
}

#[test]
#[ignore = "нет записи: tools/arbitrum_record/basin.py"]
fn basin_wells_match_node() {
    let meta = read("meta.json");
    let wells = read("wells.json");
    let conf = conf(&meta);
    let mut checked = 0usize;
    for w in wells.as_array().expect("скважины") {
        let well = addr(&w["well"]);
        if let Some(e) = w.get("error") {
            panic!("скважина {well:#x}: запись с ошибкой {e}");
        }
        // 1. Состояние — вызовы бота (слот ERC-1967 — из записи eth_getStorageAt).
        let slot = w["impl_slot"].as_str().expect("слот ERC-1967 не записан");
        let slot_impl = Address::from_word(B256::from_str(slot).expect("слово"));
        let calls = load_basin::calls(well, &conf, slot_impl);
        let raw = load_basin::decode(&answers(w, &calls), slot_impl).expect("ответы чтения");
        let st = load_basin::state(&raw, &conf, well).expect("состояние");
        let PoolState::BasinWell(d) = st else {
            panic!("не BasinWell");
        };
        // 2. Проверка — первый и второй проход («не сверено» — только скважина в точке равновесия / пустая).
        let f = judge::first(well, (d.token0, d.token1), &conf, &raw).expect("первый проход");
        match judge::second_calls(&f) {
            Ok(sc) => judge::second(&f, &answers(w, &sc)).expect("второй проход"),
            Err(why) => {
                let made = !w["shifted"].is_null();
                assert!(!made, "{well:#x}: {why}, а запись сверку сделала");
            }
        }
        // 3. getSwapOut — побайтно.
        let tok = [d.token0, d.token1];
        for q in w["get_swap_out"].as_array().expect("get_swap_out") {
            let i = q["i"].as_u64().unwrap() as usize;
            let dx = u(&q["dx"]);
            let ours = basin_well::swap_out(&d, tok[i], dx);
            match q.get("result").and_then(|h| h.as_str()) {
                Some(h) => assert_eq!(ours, Some(word(h)), "{well:#x} getSwapOut i={i} dx={dx}"),
                None => assert_eq!(ours, None, "{well:#x}: getSwapOut откатил, у нас выход"),
            }
            checked += 1;
        }
        // 4. Симуляция swapFrom.
        for s in w["swap_from_sim"].as_array().expect("swap_from_sim") {
            let i = s["i"].as_u64().unwrap() as usize;
            let dx = u(&s["dx"]);
            let ours = basin_well::swap_out(&d, tok[i], dx);
            match s["ok"].as_bool() {
                Some(true) => {
                    let ok = s["override_ok"].as_bool();
                    assert_eq!(ok, Some(true), "{well:#x}: подмена не сработала");
                    let got = word(s["result"].as_str().expect("result"));
                    assert_eq!(ours, Some(got), "{well:#x} swapFrom i={i} dx={dx}");
                }
                Some(false) => {
                    assert_eq!(ours, None, "{well:#x}: swapFrom откатил ({})", s["revert"])
                }
                None => panic!("{well:#x}: симуляция не сделана: {}", s["error"]),
            }
        }
    }
    assert!(checked > 0, "в записи нет ни одной суммы");
}
