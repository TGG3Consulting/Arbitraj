//! Файл: tests/arb_dex_camelot_v2.rs
//! Что проверяет: решение 443 (T433-F4) — Camelot V2 (camelot_v2) на РЕАЛЬНЫХ ответах своего узла Arbitrum One,
//! записанных скриптом tools/arbitrum_record/camelot_v2.py на одном блоке (tests/data/arbitrum_camelot_v2_<дата>/; берётся
//! последняя по дате папка):
//! 1. Наша математика (load_camelot::state → math/camelot_v2.rs) на резервах, комиссиях входа, stableSwap и множителях
//!    блока даёт ТОТ ЖЕ выход, что getAmountOut пары, — все записанные суммы в обе стороны (откат пары — None у нас).
//! 2. Проверка пары путём V2: поля factory/token0/token1 (verify_rules::fill/check) и реестр getPair фабрики.
//! 3. Реальные логи Sync / FeePercentUpdated / SetStableSwap — в topics() вида и разбираются events_camelot::parse;
//!    PairCreated фабрики — подпись V2.
//! 4. У всех записанных пар один код (sha256) — пары одной фабрики.
//!
//! Без записи (не помечены ignore): вид DEX — граф и сигналы, шаг SOLIDLY исполнителя, нужна только dex.factory.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{math::camelot_v2, pools::{load_camelot, events_camelot, events, families,
//! verify_rules, exec_kind}, exec::executor::kind}.

use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, B256, Bytes, U256};
use serde_json::Value;

use arb::core::types::{PoolKind, PoolState};
use arb::venues::bsc_dex::exec::executor::kind::Params;
use arb::venues::bsc_dex::math::camelot_v2::{self, Pair};
use arb::venues::bsc_dex::pools::abi_v2v3::IV2Factory;
use arb::venues::bsc_dex::pools::events_camelot::{self, Change};
use arb::venues::bsc_dex::pools::exec_kind::ExecKind;
use arb::venues::bsc_dex::pools::families::Proto;
use arb::venues::bsc_dex::pools::verify::Candidate;
use arb::venues::bsc_dex::pools::{events, families, load_camelot, verify_rules};

const NO_RECORD: &str = "нет записи: tools/arbitrum_record/camelot_v2.py";

/// Папка последней записи tests/data/arbitrum_camelot_v2_<дата>.
fn dir() -> String {
    let base = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/");
    let mut found: Vec<String> = std::fs::read_dir(base)
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.starts_with("arbitrum_camelot_v2_"))
        .collect();
    found.sort();
    format!("{base}{}/", found.last().expect(NO_RECORD))
}

fn json(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(format!("{}{name}", dir())).unwrap()).unwrap()
}

fn addr(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}

fn dec(v: &Value) -> U256 {
    U256::from_str_radix(v.as_str().unwrap(), 10).unwrap()
}

/// Слово `i` сырого ответа.
fn word(raw: &str, i: usize) -> U256 {
    let h = raw.trim_start_matches("0x");
    U256::from_str_radix(&h[64 * i..64 * (i + 1)], 16).unwrap()
}

/// Прочитанное по записанным ответам (тот же сборщик, что у чтения на узле).
fn read_of(p: &Value) -> load_camelot::Read {
    let r = p["getReserves"].as_str().unwrap();
    load_camelot::Read {
        reserve0: word(r, 0),
        reserve1: word(r, 1),
        fee0: word(r, 2).to::<u16>(),
        fee1: word(r, 3).to::<u16>(),
        stable: p["stableSwap"].as_bool().unwrap(),
        pm0: dec(&p["precisionMultiplier0"]),
        pm1: dec(&p["precisionMultiplier1"]),
    }
}

/// Состояние пары для математики — из PoolState, собранного load_camelot::state.
fn pair_of(p: &Value) -> Pair {
    let PoolState::Solidly {
        reserve0,
        reserve1,
        stable,
        decimals0,
        decimals1,
        fee,
        fee1: Some(fee1),
        ..
    } = load_camelot::state(&read_of(p)).expect("состояние")
    else {
        panic!("не Solidly с fee1")
    };
    Pair {
        reserve0,
        reserve1,
        stable,
        pm0: decimals0,
        pm1: decimals1,
        fee0: U256::from(fee.value),
        fee1: U256::from(fee1.value),
    }
}

#[test]
#[ignore = "нет записи: tools/arbitrum_record/camelot_v2.py"]
fn math_equals_pair_get_amount_out() {
    let probe = json("probe.json");
    let (mut n, mut stable, mut reverted) = (0, 0, 0);
    for p in probe["pairs"].as_array().unwrap() {
        let pair = pair_of(p);
        for g in p["getAmountOut"].as_array().unwrap() {
            let zero_for_one = g[0].as_u64().unwrap() == 0;
            let amount = dec(&g[1]);
            let ours = camelot_v2::get_amount_out(&pair, amount, zero_for_one);
            let want = (!g[2].is_null()).then(|| dec(&g[2]));
            assert_eq!(ours, want, "{} {} вход {amount}", p["pair"], g[0]);
            n += 1;
            stable += usize::from(pair.stable);
            reverted += usize::from(want.is_none());
        }
    }
    assert!(n > 0, "в записи нет ответов getAmountOut");
    println!("сверено {n}, из них стабильных {stable}, откатов {reverted}");
}

#[test]
#[ignore = "нет записи: tools/arbitrum_record/camelot_v2.py"]
fn pair_verified_by_v2_path() {
    let probe = json("probe.json");
    let fam = families::make("camelot@arbitrum", "camelot_v2").expect("вид");
    let factory = addr(&probe["factory"]);
    for p in probe["pairs"].as_array().unwrap() {
        let pool = addr(&p["pair"]);
        let (t0, t1) = (addr(&p["token0"]), addr(&p["token1"]));
        let w = |a: Address| Ok(Bytes::from(a.into_word().to_vec()));
        let answers = vec![w(addr(&p["factory"])), w(t0), w(t1)];
        let mut read = verify_rules::Read::default();
        verify_rules::fill(fam, &mut read, &answers).expect("поля пары");
        let cand = |coin: Address, quote: Address| Candidate {
            family: fam,
            pool: p["pair"].as_str().unwrap().to_string(),
            coin,
            quote,
        };
        verify_rules::check(&cand(t0, t1), factory, &read).expect("монеты и фабрика");
        assert!(verify_rules::check(&cand(pool, t1), factory, &read).is_err());
        let reg = addr(&p["getPair"]).into_word().to_vec();
        verify_rules::registered(pool, &reg).expect("реестр getPair");
        assert!(verify_rules::registered(factory, &reg).is_err());
    }
}

#[test]
#[ignore = "нет записи: tools/arbitrum_record/camelot_v2.py"]
fn real_logs_parsed() {
    let logs = json("logs.json");
    let fam = families::make("camelot@arbitrum", "camelot_v2").expect("вид");
    let topics = events::topics(fam);
    let parse = |v: &Value| {
        let log: Log = serde_json::from_value(v.clone()).expect("лог RPC");
        assert!(topics.contains(log.topic0().unwrap()));
        events_camelot::parse(&log).expect("разбор").expect("событие состояния")
    };
    let sync = logs["sync"].as_array().unwrap();
    assert!(!sync.is_empty(), "в записи нет Sync");
    for v in sync {
        let data = v["data"].as_str().unwrap();
        assert_eq!(parse(v), Change::Reserves(word(data, 0), word(data, 1)));
    }
    for v in logs["fee_percent_updated"].as_array().unwrap() {
        assert!(matches!(parse(v), Change::Fees(..)));
    }
    for v in logs["set_stable_swap"].as_array().unwrap() {
        let data = v["data"].as_str().unwrap();
        assert_eq!(parse(v), Change::Stable(word(data, 1) == U256::from(1u8)));
    }
    for v in logs["pair_created"].as_array().unwrap() {
        let t: B256 = v["topics"][0].as_str().unwrap().parse().unwrap();
        assert_eq!(t, IV2Factory::PairCreated::SIGNATURE_HASH);
    }
}

#[test]
#[ignore = "нет записи: tools/arbitrum_record/camelot_v2.py"]
fn one_code_for_all_pairs() {
    let probe = json("probe.json");
    let pairs = probe["pairs"].as_array().unwrap();
    assert!(!pairs.is_empty());
    for p in pairs {
        assert_eq!(p["code_sha256"], pairs[0]["code_sha256"], "{}", p["pair"]);
    }
}

#[test]
fn family_and_executor_step() {
    let fam = families::make("camelot@arbitrum", "camelot_v2").expect("вид");
    assert_eq!(fam.proto, Proto::CamelotV2);
    assert_eq!(fam.kind, PoolKind::Solidly);
    assert!(fam.graphable && !fam.tradable && !fam.fork);
    assert!(!fam.proto.is_solidly());
    assert_eq!(fam.executor_kind, Some(ExecKind::Solidly));
    assert_eq!(Params::Solidly.kind(), 3);
    assert!(Params::Solidly.data().is_empty());
    assert_eq!(events::topics(fam).len(), 3);
}
