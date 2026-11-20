//! Файл: tests/arbitrum_fluid_t1.rs
//! Что проверяет: решение 440 — Fluid DEX T1 (вид fluid_t1) на Arbitrum One.
//! 1. По записи своего узла (tests/data/arbitrum_fluid_t1_<дата>/, tools/arbitrum_record/fluid_t1.py; помечено
//!    #[ignore], пока записи нет): по каждому пулу резолвера состояние собирается (load_fluid::state) из тех же ответов,
//!    что читает бот; порт _swapIn (math/fluid_swap.rs) даёт ТОТ ЖЕ выход, что код пула (swapIn → 0x…dEaD, откат
//!    FluidDexSwapResult), а там, где пул откатил обмен иначе, — None; у пулов без VERSION() выход со всеми проверками
//!    (math/fluid_check.rs) = estimateSwapIn резолвера (0 ↔ None).
//! 2. Без записи (факты кода): вид fluid_t1 — в графе, без своего пути, шаг FLUID = 22 только с исполнителя v4, вид ядра
//!    SmartColDebt, 22 topic0 событий пула; раскладка данных шага.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{pools::{load_fluid, abi_fluid, families, exec_kind, exec_callback},
//! math::{fluid_swap, fluid_check}, exec::executor::{code, kind}}.

use std::path::{Path, PathBuf};

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;

use arb::core::types::{PoolKind, PoolState};
use arb::venues::bsc_dex::exec::executor::code::Version;
use arb::venues::bsc_dex::math::{fluid_check, fluid_swap};
use arb::venues::bsc_dex::pools::abi_fluid::{IFluidDexReservesResolver as R, IFluidDexT1 as P};
use arb::venues::bsc_dex::pools::exec_kind::{self, ExecKind};
use arb::venues::bsc_dex::pools::{abi_fluid, exec_callback, families, load_fluid};

/// Селектор отката FluidDexSwapResult(uint256) — keccak256 подписи (poolT1/coreModule/events и iDexT1.sol).
const SWAP_RESULT: &str = "0xb3bfda99";

/// Последняя по имени папка записи tests/data/arbitrum_fluid_t1_*. Паника — записи нет.
fn dir() -> PathBuf {
    let base = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data"));
    let mut found: Vec<PathBuf> = std::fs::read_dir(&base)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            name.starts_with("arbitrum_fluid_t1_")
        })
        .collect();
    found.sort();
    found.pop().expect("нет записи: tools/arbitrum_record/fluid_t1.py")
}

/// JSON из файла записи.
fn json(p: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

/// Байты ответа eth_call (result). None — вызов откатился.
fn result(v: &Value) -> Option<Bytes> {
    v.get("result")?.as_str()?.parse().ok()
}

/// Разбор ответа функции. Паника — не по ABI.
fn dec<C: SolCall>(b: &[u8]) -> C::Return {
    C::abi_decode_returns(b).unwrap()
}

/// Состояние пула из записи. Принимает: запись пула, обёртку, время блока. Возвращает: состояние или текст причины.
fn state_of(rec: &Value, wrapped: Address, ts: u64) -> Result<PoolState, String> {
    let need = |k: &str| result(&rec[k]).ok_or_else(|| format!("{k}: откат"));
    let raw = load_fluid::Raw {
        reserves: dec::<R::getPoolReservesAdjustedCall>(&need("reserves")?),
        constants: dec::<P::constantsViewCall>(&need("constantsView")?),
        scales: dec::<P::constantsView2Call>(&need("constantsView2")?),
        vars: dec::<P::readFromStorageCall>(&need("vars")?),
        vars2: dec::<P::readFromStorageCall>(&need("vars2")?),
        versioned: result(&rec["version"]).is_some(),
        block_ts: U256::from(ts),
    };
    load_fluid::state(&raw, Some(wrapped))
}

/// Выход пула из отката swapIn → 0x…dEaD. None — пул откатил обмен другой ошибкой.
fn pool_out(v: &Value) -> Option<U256> {
    let data = v.get("error")?.get("data")?.as_str()?;
    let body = data.strip_prefix(SWAP_RESULT)?;
    U256::from_str_radix(body, 16).ok()
}

#[test]
#[ignore = "нет записи: tools/arbitrum_record/fluid_t1.py"]
fn math_matches_pool_and_resolver_on_record() {
    let d = dir();
    let block = json(&d.join("block.json"));
    let wrapped: Address = block["wrapped"].as_str().unwrap().parse().unwrap();
    let ts = block["timestamp"].as_u64().unwrap();
    let (mut same, mut checked, mut res_same, mut res_checked) = (0, 0, 0, 0);
    let mut skipped: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(&d).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_str().unwrap().to_string();
        if !name.starts_with("pool_0x") {
            continue;
        }
        let rec = json(&path);
        let s = match state_of(&rec, wrapped, ts) {
            Ok(PoolState::SmartColDebt(s)) => s,
            Ok(_) => panic!("{name}: состояние не SmartColDebt"),
            Err(why) => {
                skipped.push(format!("{name}: {why}"));
                continue;
            }
        };
        for q in rec["quotes"].as_array().unwrap() {
            let dir0 = q["swap0to1"].as_bool().unwrap();
            let amt = U256::from_str_radix(q["amountIn"].as_str().unwrap(), 10).unwrap();
            let ours = fluid_swap::swap_in(&s, dir0, amt).map(|o| o.amount_out);
            checked += 1;
            let theirs = pool_out(&q["pool"]);
            if ours == theirs {
                same += 1;
            } else {
                eprintln!("{name} {dir0} {amt}: {ours:?} ≠ {theirs:?}");
            }
            if s.versioned {
                continue;
            }
            let Some(r) = result(&q["resolver"]) else {
                continue;
            };
            let v = dec::<R::estimateSwapInCall>(&r);
            res_checked += 1;
            // Резолвер отвечает 0 и при откате, и при нулевом выходе — у нас оба случая «нет выхода».
            let ours = fluid_check::quote_in(&s, dir0, amt).filter(|x| !x.is_zero());
            if ours == (!v.is_zero()).then_some(v) {
                res_same += 1;
            } else {
                eprintln!("{name} {dir0} {amt}: резолвер {v}");
            }
        }
    }
    eprintln!("пул {same}/{checked}, резолвер {res_same}/{res_checked}");
    // Ревью 440: пул записи без состояния — не молча: причина в выводе, пропусков быть не должно.
    for w in &skipped {
        eprintln!("пропущен {w}");
    }
    assert!(skipped.is_empty(), "пулы без состояния: {}", skipped.len());
    assert!(checked > 0, "в записи нет обменов");
    assert_eq!(same, checked, "порт _swapIn ≠ пулу");
    assert_eq!(res_same, res_checked, "≠ estimateSwapIn");
}

#[test]
fn family_kind_and_step() {
    let (pk, proto, graphable, tradable, fork) = families::spec("fluid_t1").unwrap();
    assert_eq!(pk, PoolKind::SmartColDebt);
    assert_eq!(proto, families::Proto::FluidT1);
    assert!(graphable && !tradable && !fork);
    assert_eq!(exec_kind::of("fluid_t1"), Some(ExecKind::Fluid));
    assert_eq!(ExecKind::Fluid.name(), "FLUID");
    assert_eq!(exec_callback::since(ExecKind::Fluid), Version::V4);
    let absent = exec_callback::step_absent(ExecKind::Fluid, Version::V3);
    assert!(absent.is_some());
    assert_eq!(Version::parse("v4"), Some(Version::V4));
    assert_eq!(abi_fluid::topics().len(), 22);
    assert_eq!(abi_fluid::liquidity_topics().len(), 10);
}
