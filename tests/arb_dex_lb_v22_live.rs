//! Файл: tests/arb_dex_lb_v22_live.rs
//! Что проверяет: решение 446 — Liquidity Book V2.2 (lb_v22, LFJ V2.2 на Arbitrum One) на РЕАЛЬНЫХ ответах своего узла
//! Arbitrum, записанных tools/arbitrum_record/lb_v22.py в tests/data/arbitrum_lb_v22_<дата>/ (берётся последняя папка).
//! Записи пока нет — тесты на данных помечены #[ignore]; после записи метка снимается.
//! 1. Наша математика (load_lb::decode → math/lb_swap.rs swap_out_v22) на записанном состоянии и времени блока даёт ТОТ
//!    ЖЕ ответ, что getSwapOut пула (остаток входа, выход, комиссия); откат getSwapOut — у нас None.
//! 2. Проверка пула: первый и второй проход V2.1 (verify_lb) и третий проход V2.2 (verify_lb22: implementation(),
//!    крючки) по записанным ответам; записанная calldata = ABI кода бота (селекторы скрипта сверены).
//! 3. Условие пулов (exec/pool_cond.rs, слоты 3 и 4): слово 3 — _parameters (активный бин в битах 232..255 =
//!    getActiveId), слово 4 — _reserves (младшие 128 бит = reserveX, старшие = reserveY getReserves).
//! 4. События: Swap пулов разбирает events_lb (бин = активный после цикла), HooksParametersSet — topic0 abi_lb22.
//!
//! Без записи (не данные — факты кода): вид lb_v22 (LbBin, шаг LB) и topic0 событий V2.2.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{math::lb_swap, pools::{abi_lb, abi_lb22, bins, events_lb, events_lb22,
//! exec_kind, families, lb_hooks, load_lb, verify, verify_lb, verify_lb22}}.

use std::collections::BTreeMap;
use std::str::FromStr;

use alloy::sol_types::{SolCall, SolEvent};
use alloy_primitives::{Address, B256, Bytes, LogData, U256, keccak256};
use serde_json::Value;

use arb::core::types::{LbBinData, PoolKind};
use arb::venues::bsc_dex::math::lb_swap;
use arb::venues::bsc_dex::pools::abi_lb::{self, ILBPair};
use arb::venues::bsc_dex::pools::abi_lb22::{self, ILBPair22};
use arb::venues::bsc_dex::pools::bins::BinMap;
use arb::venues::bsc_dex::pools::exec_kind::{self, ExecKind};
use arb::venues::bsc_dex::pools::families::{self, Proto};
use arb::venues::bsc_dex::pools::verify::Candidate;
use arb::venues::bsc_dex::pools::{
    events_lb, events_lb22, lb_hooks, load_lb, verify_lb, verify_lb22,
};

const NO_DATA: &str = "нет записи: tools/arbitrum_record/lb_v22.py";

/// Последняя папка записи tests/data/arbitrum_lb_v22_<дата>. Ошибки: паника — записи нет.
fn dir() -> String {
    let base = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/");
    let mut list: Vec<String> = std::fs::read_dir(base)
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.starts_with("arbitrum_lb_v22_"))
        .collect();
    list.sort();
    format!("{base}{}/", list.last().expect(NO_DATA))
}

fn json(name: &str) -> Value {
    let text = std::fs::read_to_string(format!("{}{name}", dir())).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn hex(v: &Value) -> Bytes {
    Bytes::from_str(v.as_str().unwrap()).unwrap()
}

fn u128s(v: &Value) -> u128 {
    v.as_str().unwrap().parse().unwrap()
}

/// Ответ вызова записи: байты или текст отката.
fn answer(c: &Value) -> Result<Bytes, String> {
    match c.get("out") {
        Some(o) => Ok(hex(o)),
        None => Err(c["revert"].as_str().unwrap_or("откат").to_string()),
    }
}

fn out(p: &Value, name: &str) -> Bytes {
    answer(&p["calls"][name]).unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// Пулы записи с прочитанным состоянием (getTokenX и др. не откатились).
fn pools(probe: &Value) -> Vec<&Value> {
    probe["pools"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p.get("bins").is_some())
        .collect()
}

fn token(p: &Value, name: &str) -> Address {
    ILBPair::getTokenXCall::abi_decode_returns(&out(p, name)).unwrap()
}

/// Состояние пула тем же разборщиком, что у чтения на узле (load_lb::decode).
fn state(probe: &Value, p: &Value) -> LbBinData {
    let a = [
        out(p, "getActiveId"),
        out(p, "getStaticFeeParameters"),
        out(p, "getVariableFeeParameters"),
    ];
    let step = ILBPair::getBinStepCall::abi_decode_returns(&out(p, "getBinStep")).unwrap();
    let x_is_token0 = token(p, "getTokenX") < token(p, "getTokenY");
    let ts = probe["timestamp"].as_u64().unwrap();
    load_lb::decode(&a, x_is_token0, u32::from(step), ts).unwrap()
}

fn bins(p: &Value, step: u32) -> BinMap {
    let mut m = BTreeMap::new();
    for (k, v) in p["bins"].as_object().unwrap() {
        m.insert(k.parse::<u32>().unwrap(), (u128s(&v[0]), u128s(&v[1])));
    }
    BinMap {
        step,
        reserves: m.into(),
        range: (0, 0xff_ffff),
    }
}

#[test]
#[ignore = "нет записи: tools/arbitrum_record/lb_v22.py"]
fn formula_equals_get_swap_out() {
    let probe = json("probe.json");
    let ts = probe["timestamp"].as_u64().unwrap();
    let mut n = 0;
    for p in pools(&probe) {
        if p["bins_complete"] != true {
            continue;
        }
        let d = state(&probe, p);
        let b = bins(p, d.bin_step);
        for q in p["quotes"].as_array().unwrap() {
            let for_y = q["swapForY"].as_bool().unwrap();
            let r = lb_swap::swap_out_v22(&d, &b, ts, for_y, u128s(&q["amountIn"]));
            match answer(q) {
                Ok(o) => {
                    let w = ILBPair::getSwapOutCall::abi_decode_returns(&o).unwrap();
                    let r = r.expect("расчёт");
                    let want = (w.amountInLeft, w.amountOut, w.fee);
                    assert_eq!((r.in_left, r.out, r.fee), want, "{} {q}", p["pool"]);
                    assert_eq!(r.complete, r.in_left == 0);
                }
                Err(why) => assert!(r.is_none(), "{} {q}: откат пула ({why})", p["pool"]),
            }
            n += 1;
        }
    }
    assert!(n > 0, "нет пулов с полным обходом бинов");
}

#[test]
#[ignore = "нет записи: tools/arbitrum_record/lb_v22.py"]
fn verify_and_recorded_calldata() {
    let probe = json("probe.json");
    let fam = families::make("traderjoe-v2-2-arbitrum@arbitrum", "lb_v22").expect("вид");
    let factory = Address::from_str(probe["factory"].as_str().unwrap()).unwrap();
    for p in pools(&probe) {
        let pool = Address::from_str(p["pool"].as_str().unwrap()).unwrap();
        // Записанная calldata = ABI кода бота.
        let c = &p["calls"];
        let factory_data = Bytes::from(ILBPair::getFactoryCall {}.abi_encode());
        assert_eq!(hex(&c["getFactory"]["data"]), factory_data);
        let active_data = Bytes::from(ILBPair::getActiveIdCall {}.abi_encode());
        assert_eq!(hex(&c["getActiveId"]["data"]), active_data);
        let imp_data = Bytes::from(ILBPair22::implementationCall {}.abi_encode());
        assert_eq!(hex(&c["implementation"]["data"]), imp_data);
        let hooks_data = Bytes::from(ILBPair22::getLBHooksParametersCall {}.abi_encode());
        assert_eq!(hex(&c["getLBHooksParameters"]["data"]), hooks_data);
        let (x, y) = (token(p, "getTokenX"), token(p, "getTokenY"));
        let cand = Candidate {
            family: fam,
            pool: p["pool"].as_str().unwrap().into(),
            coin: x,
            quote: y,
        };
        let names = ["getFactory", "getTokenX", "getTokenY", "getBinStep"];
        let first: Vec<Result<Bytes, String>> = names.iter().map(|n| answer(&c[n])).collect();
        let f = verify_lb::first_pass(&cand, &(pool, factory), &first).expect("первый проход");
        let reg = verify_lb::second_call(&(pool, factory), &f);
        assert_eq!(hex(&p["registry"]["data"]), reg.1);
        let found =
            verify_lb::decide(&cand, (pool, factory), f, &answer(&p["registry"])).expect("реестр");
        assert_eq!(found.bin_step, Some(u32::from(f.2)));
        // Третий проход: реализация пула в списке — пул берётся (если крючок обмена допущен); не в списке — отказ.
        let third = [answer(&c["implementation"]), answer(&c["getLBHooksParameters"])];
        let imp_out = out(p, "implementation");
        let imp = ILBPair22::implementationCall::abi_decode_returns(&imp_out).unwrap();
        let hooks_out = out(p, "getLBHooksParameters");
        let hp = ILBPair22::getLBHooksParametersCall::abi_decode_returns(&hooks_out).unwrap();
        let hook = lb_hooks::hooks_address(hp);
        assert!(verify_lb22::third_pass(&third, &[imp], Some(&[hook][..])).is_ok());
        assert!(verify_lb22::third_pass(&third, &[], Some(&[hook][..])).is_err());
        // Без списка крючков: пул без флага обмена берётся, с флагом — отказ (то же, что lb_hooks::decide).
        assert_eq!(
            verify_lb22::third_pass(&third, &[imp], None).is_ok(),
            lb_hooks::decide(hp, None).is_ok()
        );
    }
}

#[test]
#[ignore = "нет записи: tools/arbitrum_record/lb_v22.py"]
fn pool_cond_slots_3_and_4() {
    let probe = json("probe.json");
    for p in pools(&probe) {
        let w = |i: &str| U256::from_str(p["storage"][i].as_str().unwrap()).unwrap();
        let active = ILBPair::getActiveIdCall::abi_decode_returns(&out(p, "getActiveId")).unwrap();
        let id = (w("3") >> 232usize) & U256::from(0xff_ffffu32);
        assert_eq!(id, U256::from(active.to::<u32>()), "{}", p["pool"]);
        let r = ILBPair22::getReservesCall::abi_decode_returns(&out(p, "getReserves")).unwrap();
        let mask = (U256::from(1u8) << 128usize) - U256::from(1u8);
        assert_eq!(w("4") & mask, U256::from(r.reserveX), "{}", p["pool"]);
        assert_eq!(w("4") >> 128usize, U256::from(r.reserveY), "{}", p["pool"]);
    }
}

#[test]
#[ignore = "нет записи: tools/arbitrum_record/lb_v22.py"]
fn recorded_logs_decode() {
    let logs = json("logs.json");
    let swap = ILBPair::Swap::SIGNATURE_HASH;
    let mut n = 0;
    for l in logs["pool_logs"].as_array().unwrap() {
        let topics: Vec<B256> = l["topics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| B256::from_str(t.as_str().unwrap()).unwrap())
            .collect();
        if topics[0] != swap {
            continue;
        }
        let data = LogData::new_unchecked(topics.clone(), hex(&l["data"]));
        let (ids, active) = events_lb::bins_of(topics[0], &data).unwrap().unwrap();
        assert_eq!(Some(ids[0]), active);
        n += 1;
    }
    for l in logs["hooks_set"].as_array().unwrap() {
        let t0 = B256::from_str(l["topics"][0].as_str().unwrap()).unwrap();
        assert_eq!(t0, abi_lb22::hooks_set_topic());
        assert!(events_lb22::topics().contains(&t0));
    }
    eprintln!("Swap логов: {n}");
}

/// Факты кода (без записи): вид lb_v22 и события V2.2.
#[test]
fn kind_and_topics() {
    let f = families::make("traderjoe-v2-2-arbitrum@arbitrum", "lb_v22").expect("вид");
    assert_eq!(f.kind, PoolKind::LbBin);
    assert_eq!(f.proto, Proto::LbV22);
    assert!(f.graphable && !f.tradable && !f.fork);
    assert_eq!(exec_kind::of("lb_v22"), Some(ExecKind::Lb));
    assert_eq!(
        abi_lb22::hooks_set_topic(),
        keccak256("HooksParametersSet(address,bytes32)")
    );
    let mut want = abi_lb::topics();
    want.push(abi_lb22::hooks_set_topic());
    assert_eq!(events_lb22::topics(), want);
}
