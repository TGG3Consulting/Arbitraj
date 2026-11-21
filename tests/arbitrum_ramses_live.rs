//! Файл: tests/arbitrum_ramses_live.rs
//! Что проверяет: решение 483 — три DEX Ramses на Arbitrum как варианты видов бота: Ramses CL (v3_fork_ramses),
//! Ramses CL V2 (slipstream_ramses), Ramses Legacy V2 (solidly_v1_ramses).
//! Без записи: вид, протокол, вариант и шаг исполнителя по dex.kind; правило «выход пары − 1» легаси (Pair.sol L457).
//! На записи своего узла (tests/data/arbitrum_ramses_<дата>/record.json, tools/arbitrum_record/ramses.py; #[ignore] до
//! записи): (1) наша математика V3 (math/swap_cl, комиссия currentFee() / fee() пула, карта тиков из tickBitmap +
//! ticks() пула) = проба обмена swap самого пула (дельты из отката обратного вызова) на каждой записанной сумме; у линии
//! V3 ещё = QuoterV2; (2) математика легаси (net_in по fee() над 1 000 000, solidly_v1_math, variant_out) = getAmountOut
//! пары; (3) реестры фабрик (getPool / getPair / isPair) и factory() пулов; (4) разбор записанных событий FeeAdjustment,
//! Mint (линия V3 — с index), Sync(uint112, uint112), PairCreated.
//! Кто вызывает: cargo test (CI).
//! От чего зависит: arb::venues::bsc_dex::{math::{swap_cl, solidly_fee, solidly_v1_math}, pools::{families, exec_kind,
//! family_variant, abi_ramses, abi_v2v3, ramses, ticks}}, core/types (SolidlyFee, FeeRule).

use std::collections::BTreeMap;

use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, B256, Bytes, LogData, U256};
use serde_json::Value;

use arb::core::types::{FeeRule, PoolKind, SolidlyFee};
use arb::venues::bsc_dex::math::solidly_fee;
use arb::venues::bsc_dex::math::solidly_v1_math::{self, variant_out, variant_want};
use arb::venues::bsc_dex::math::swap_cl::{self, ClState, Version};
use arb::venues::bsc_dex::pools::abi_ramses::{IRamsesPoolFee, IRamsesV3Pool};
use arb::venues::bsc_dex::pools::abi_v2v3::{IV2Factory, IV2Pair, IV3Pool};
use arb::venues::bsc_dex::pools::exec_kind::{self, ExecKind};
use arb::venues::bsc_dex::pools::families::{self, Proto};
use arb::venues::bsc_dex::pools::family_variant::Variant;
use arb::venues::bsc_dex::pools::ramses;
use arb::venues::bsc_dex::pools::ticks::{TickData, TickMap};

const NO_RECORD: &str = "нет записи: tools/arbitrum_record/ramses.py";
/// FEE_DENOM пары легаси (Pair.sol L63, PairFactory 0xADd3…Dc3B, Sourcify 42161 match).
const LEGACY_DENOM: u32 = 1_000_000;

/// Вид, протокол, вариант и шаг исполнителя по dex.kind (исходники — шапка pools/abi_ramses.rs).
#[test]
fn kinds_variants_steps() {
    let cl = (
        PoolKind::V3,
        Proto::UniswapV3,
        Variant::RamsesCl,
        ExecKind::V3,
    );
    let v3 = (
        PoolKind::V3,
        Proto::Slipstream,
        Variant::RamsesClV2,
        ExecKind::V3,
    );
    let leg = (
        PoolKind::Solidly,
        Proto::SolidlyV1,
        Variant::RamsesLegacy,
        ExecKind::Solidly,
    );
    let cases = [
        ("v3_fork_ramses", cl),
        ("slipstream_ramses", v3),
        ("solidly_v1_ramses", leg),
    ];
    for (kind, (pk, proto, variant, step)) in cases {
        let (k, p, ..) = families::spec(kind).unwrap();
        assert_eq!((k, p), (pk, proto), "{kind}");
        assert_eq!(exec_kind::of(kind), Some(step), "{kind}");
        let f = families::make(&format!("t483-{kind}@arbitrum"), kind).unwrap();
        assert_eq!(f.variant, variant, "{kind}");
        assert_eq!(f.executor_kind, Some(step), "{kind}");
        assert!(ramses::handles(f), "{kind}");
        assert_eq!(ramses::is_cl(f), variant != Variant::RamsesLegacy, "{kind}");
    }
    // Прежние виды — без варианта Ramses.
    for kind in ["v3_fork", "slipstream", "solidly_v1"] {
        let f = families::make(&format!("t483-{kind}"), kind).unwrap();
        assert!(!ramses::handles(f), "{kind}");
    }
    let cl = families::make("t483-cl@arbitrum", "v3_fork_ramses").unwrap();
    let t = ramses::topics(cl);
    assert!(t.contains(&IRamsesPoolFee::FeeAdjustment::SIGNATURE_HASH));
    assert!(t.contains(&IV3Pool::Mint::SIGNATURE_HASH));
    let v3 = families::make("t483-v3@arbitrum", "slipstream_ramses").unwrap();
    let t = ramses::topics(v3);
    assert!(t.contains(&IRamsesV3Pool::Mint::SIGNATURE_HASH));
    assert!(!t.contains(&IV3Pool::Mint::SIGNATURE_HASH));
    let leg = families::make("t483-leg@arbitrum", "solidly_v1_ramses").unwrap();
    assert_eq!(ramses::topics(leg), vec![IV2Pair::Sync::SIGNATURE_HASH]);
    assert_eq!(
        ramses::created_topic(leg),
        Some(IV2Factory::PairCreated::SIGNATURE_HASH)
    );
    assert_eq!(ramses::created_topic(cl), None);
}

/// Pair.sol L457: `_getAmountOut(...) - 1` (Solidity 0.8 — при нуле откат); точный выход — формуле нужен выход + 1.
#[test]
fn legacy_out_minus_one() {
    let one = U256::from(1u8);
    assert_eq!(
        variant_out(Variant::RamsesLegacy, U256::from(10u8)),
        Some(U256::from(9u8))
    );
    assert_eq!(variant_out(Variant::RamsesLegacy, U256::ZERO), None);
    assert_eq!(variant_out(Variant::Base, U256::ZERO), Some(U256::ZERO));
    assert_eq!(
        variant_want(Variant::RamsesLegacy, one),
        Some(U256::from(2u8))
    );
    assert_eq!(variant_want(Variant::Base, one), Some(one));
}

fn record() -> Value {
    let base = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/");
    let mut found: Vec<String> = std::fs::read_dir(base)
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.starts_with("arbitrum_ramses_"))
        .collect();
    found.sort();
    let dir = format!("{base}{}/record.json", found.last().expect(NO_RECORD));
    serde_json::from_str(&std::fs::read_to_string(dir).unwrap()).unwrap()
}

fn hex(s: &str) -> Vec<u8> {
    alloy_primitives::hex::decode(s.trim_start_matches("0x")).unwrap()
}

/// Слово i ответа eth_call. None — откат или короткий ответ.
fn w(v: &Value, i: usize) -> Option<U256> {
    let s = v.get("result")?.as_str()?.strip_prefix("0x")?;
    U256::from_str_radix(s.get(i * 64..(i + 1) * 64)?, 16).ok()
}

/// Слово i со знаком (int24 / int128 в слове ABI — расширение знака до 256 бит).
fn signed(v: &Value, i: usize) -> Option<i128> {
    let x = w(v, i)?;
    Some(if x.bit(255) {
        -((!x + U256::from(1u8)).to::<i128>())
    } else {
        x.to::<i128>()
    })
}

fn addr(v: &Value) -> Address {
    Address::from_word(B256::from(w(v, 0).unwrap()))
}

fn u(v: &Value) -> U256 {
    U256::from_str_radix(v.as_str().unwrap(), 10).unwrap()
}

/// Карта тиков пула из записанных слов tickBitmap и ticks() (liquidityGross — слово 0, liquidityNet — слово 1 у обеих
/// линий: RamsesV2Pool.sol L345-L346, RamsesV3Pool.sol L901-L902).
fn map_of(p: &Value, spacing: i32) -> TickMap {
    let m = &p["map"];
    let words = m["words"].as_array().unwrap();
    let mut ticks = BTreeMap::new();
    for t in m["ticks"].as_array().unwrap() {
        let gross = w(&t["raw"], 0).unwrap().to::<u128>();
        let net = signed(&t["raw"], 1).unwrap();
        ticks.insert(t["tick"].as_i64().unwrap() as i32, TickData { gross, net });
    }
    let first = words.first().unwrap()["word"].as_i64().unwrap() as i16;
    let last = words.last().unwrap()["word"].as_i64().unwrap() as i16;
    TickMap {
        spacing,
        ticks: ticks.into(),
        words: (first, last),
    }
}

/// (1): математика V3 = проба обмена пула; линия V3 — ещё = QuoterV2. Сумма за пределами прочитанных слов — только
/// «частичный выход не больше пробы» (как sheepdex-тест).
#[test]
#[ignore = "нет записи: tools/arbitrum_record/ramses.py"]
fn cl_math_equals_pool_swap() {
    let r = record();
    let mut full = 0;
    for (line, fee_key) in [("cl", "currentFee()"), ("v3", "fee()")] {
        for p in r[line].as_array().unwrap() {
            let spacing = signed(&p["tickSpacing()"], 0).unwrap() as i32;
            let map = map_of(p, spacing);
            let s = ClState {
                sqrt_price: w(&p["slot0()"], 0).unwrap(),
                tick: signed(&p["slot0()"], 1).unwrap() as i32,
                liquidity: w(&p["liquidity()"], 0).unwrap().to::<u128>(),
                fee: w(&p[fee_key], 0).unwrap().to::<u32>(),
                ticks: &map,
                version: Version::V3,
            };
            for q in p["swaps"].as_array().unwrap() {
                let (zfo, amount) = (q["zeroForOne"].as_bool().unwrap(), u(&q["amountIn"]));
                let pr = &q["probe"];
                let (Some(a0), Some(a1)) = (pr["amount0"].as_str(), pr["amount1"].as_str()) else {
                    panic!("{line} {}: проба без дельт: {pr}", p["pool"]);
                };
                let (a0, a1): (i128, i128) = (a0.parse().unwrap(), a1.parse().unwrap());
                let (used, out) = if zfo { (a0, -a1) } else { (a1, -a0) };
                let m = swap_cl::swap(&s, zfo, true, amount).expect("расчёт");
                if !m.complete {
                    assert!(
                        m.amount_out <= U256::from(out as u128),
                        "{line} {}",
                        p["pool"]
                    );
                    continue;
                }
                full += 1;
                assert_eq!(
                    m.amount_in,
                    U256::from(used as u128),
                    "{line} {} вход",
                    p["pool"]
                );
                assert_eq!(
                    m.amount_out,
                    U256::from(out as u128),
                    "{line} {} выход",
                    p["pool"]
                );
                if line == "v3" {
                    let quoter = w(&q["quoter"], 0);
                    assert_eq!(quoter, Some(m.amount_out), "{line} {} QuoterV2", p["pool"]);
                }
            }
        }
    }
    assert!(full > 0, "ни одной полной сверки");
}

/// (2): математика легаси = getAmountOut пары на каждой сумме.
#[test]
#[ignore = "нет записи: tools/arbitrum_record/ramses.py"]
fn legacy_math_equals_get_amount_out() {
    let r = record();
    let mut n = 0;
    for p in r["legacy"]["pairs"].as_array().unwrap() {
        let m = &p["metadata()"];
        let word = |i: usize| w(m, i).unwrap();
        let (dec0, dec1, r0, r1) = (word(0), word(1), word(2), word(3));
        let stable = !word(4).is_zero();
        let t0 = Address::from_word(B256::from(word(5)));
        let fee = SolidlyFee {
            value: w(&p["fee()"], 0).unwrap().to::<u32>(),
            rule: FeeRule::Numerator {
                denominator: LEGACY_DENOM,
            },
        };
        for q in p["quotes"].as_array().unwrap() {
            let Some(want) = w(&q["out"], 0) else {
                continue;
            };
            let tin: Address = q["tokenIn"].as_str().unwrap().parse().unwrap();
            let (ri, ro, di, dout) = if tin == t0 {
                (r0, r1, dec0, dec1)
            } else {
                (r1, r0, dec1, dec0)
            };
            let net = solidly_fee::net_in(u(&q["amountIn"]), &fee).unwrap();
            let out = if stable {
                solidly_v1_math::stable_amount_out(net, ri, ro, di, dout, U256::ZERO)
            } else {
                solidly_v1_math::volatile_amount_out(net, ri, ro, U256::ZERO)
            }
            .and_then(|o| variant_out(Variant::RamsesLegacy, o));
            assert_eq!(out, Some(want), "{} {}", p["pair"], q["amountIn"]);
            n += 1;
        }
    }
    assert!(n > 0, "нет котировок пар легаси");
}

/// (3): реестры фабрик и factory() пулов.
#[test]
#[ignore = "нет записи: tools/arbitrum_record/ramses.py"]
fn registries_match() {
    let r = record();
    let cl_factory: Address = "0xAA2cd7477c451E703f3B9Ba5663334914763edF8"
        .parse()
        .unwrap();
    let v3_factory: Address = "0xd0019e86edB35E1fedaaB03aED5c3c60f115d28b"
        .parse()
        .unwrap();
    for (line, f) in [("cl", cl_factory), ("v3", v3_factory)] {
        for p in r[line].as_array().unwrap() {
            let pool: Address = p["pool"].as_str().unwrap().parse().unwrap();
            assert_eq!(addr(&p["factory()"]), f, "{line} {pool}");
            assert_eq!(addr(&p["getPool"]), pool, "{line} {pool}");
        }
    }
    for p in r["legacy"]["pairs"].as_array().unwrap() {
        let pair: Address = p["pair"].as_str().unwrap().parse().unwrap();
        assert_eq!(w(&p["isPair"], 0), Some(U256::from(1u8)), "{pair}");
        assert_eq!(addr(&p["getPair"]), pair, "{pair}");
    }
}

/// (4): записанные события разбираются ABI бота.
#[test]
#[ignore = "нет записи: tools/arbitrum_record/ramses.py"]
fn logs_decode() {
    let r = record();
    let l = &r["logs"];
    let data = |x: &Value| {
        let topics: Vec<B256> = x["topics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| B256::from_slice(&hex(t.as_str().unwrap())))
            .collect();
        LogData::new(topics, Bytes::from(hex(x["data"].as_str().unwrap()))).unwrap()
    };
    let all = |k: &str| l[k].as_array().cloned().unwrap_or_default();
    for x in all("cl").iter().chain(all("v3").iter()) {
        let d = data(x);
        let t0 = d.topics()[0];
        if t0 == IRamsesPoolFee::FeeAdjustment::SIGNATURE_HASH {
            IRamsesPoolFee::FeeAdjustment::decode_log_data(&d).expect("FeeAdjustment");
        } else if t0 == IRamsesV3Pool::Mint::SIGNATURE_HASH {
            IRamsesV3Pool::Mint::decode_log_data(&d).expect("Mint (линия V3)");
        } else {
            IV3Pool::Mint::decode_log_data(&d).expect("Mint (старая линия)");
        }
    }
    for x in all("sync") {
        IV2Pair::Sync::decode_log_data(&data(&x)).expect("Sync(uint112, uint112)");
    }
    for x in all("pairCreated") {
        IV2Factory::PairCreated::decode_log_data(&data(&x)).expect("PairCreated");
    }
}
